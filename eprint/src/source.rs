//! Pluggable PDF byte sources.
//!
//! A paper's PDF can come from several places. eprint.iacr.org's PDF endpoint
//! is currently behind a Cloudflare challenge (HTTP 403), so today PDFs arrive
//! out-of-band: a human downloads them in a browser and a companion watcher
//! drops them into the downloads dir under the canonical `<year>-<num>.pdf`
//! name. A **requester-pays S3 bucket** is planned as another source.
//!
//! Rather than special-casing each, acquisition goes through an ordered list of
//! [`PdfSource`]s. [`acquire`] tries each in turn and takes the first that
//! yields bytes. Adding S3 later is a matter of writing one more `PdfSource`
//! and inserting it into [`build_sources`] — no churn in the fetch path.
//!
//! Metadata (title/abstract/bibtex via the landing page, version datestamps via
//! OAI-PMH) is a separate concern handled by the caller; sources only produce
//! PDF bytes.

use crate::cli::Context;
use crate::id::PaperId;
use crate::net;
use crate::version::Canonical;
use anyhow::{Context as _, Result};
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, warn};

/// What the caller wants: a specific version of a paper's PDF.
pub struct PdfRequest<'a> {
    pub id: PaperId,
    pub version: &'a Canonical,
    /// True iff `version` is the paper's current version. Some sources (the
    /// downloads dir; possibly the S3 bucket) only ever hold the current PDF.
    pub is_current: bool,
}

/// Bytes plus provenance, so the caller can label the action it took.
pub struct Acquired {
    pub source: &'static str,
    pub network: bool,
    pub bytes: Vec<u8>,
}

/// A place PDF bytes can come from.
#[async_trait]
pub trait PdfSource: Send + Sync {
    fn name(&self) -> &'static str;
    /// Whether this source makes network requests (skipped under `--offline`).
    fn is_network(&self) -> bool;
    /// `Ok(None)` means "I don't have this one — try the next source".
    async fn fetch(&self, req: &PdfRequest<'_>) -> Result<Option<Vec<u8>>>;
}

/// The browser-delivered downloads dir: holds only the *current* PDF, named
/// canonically by the companion watcher.
pub struct DownloadsSource {
    pub dir: PathBuf,
}

#[async_trait]
impl PdfSource for DownloadsSource {
    fn name(&self) -> &'static str {
        "downloads"
    }
    fn is_network(&self) -> bool {
        false
    }
    async fn fetch(&self, req: &PdfRequest<'_>) -> Result<Option<Vec<u8>>> {
        if !req.is_current {
            return Ok(None); // the dir only carries the current PDF
        }
        match crate::downloads::local_pdf(&self.dir, req.id) {
            Some(path) => {
                let bytes = tokio::fs::read(&path)
                    .await
                    .with_context(|| format!("reading downloaded PDF {}", path.display()))?;
                Ok(Some(bytes))
            }
            None => Ok(None),
        }
    }
}

/// Direct fetch from eprint.iacr.org.
///
/// NOTE: the PDF endpoints are currently behind a Cloudflare managed challenge
/// and return HTTP 403 from every IP, so this source effectively always errors
/// today. It is retained (not stripped) as the lowest-priority fallback for
/// if/when direct access returns, and because the OAI/RSS endpoints on the same
/// host are still reachable.
pub struct EprintHttpSource {
    client: reqwest::Client,
    rl: Arc<net::RateLimiter>,
}

#[async_trait]
impl PdfSource for EprintHttpSource {
    fn name(&self) -> &'static str {
        "eprint-http"
    }
    fn is_network(&self) -> bool {
        true
    }
    async fn fetch(&self, req: &PdfRequest<'_>) -> Result<Option<Vec<u8>>> {
        let url = if req.is_current {
            req.id.pdf_url()
        } else {
            req.id.historical_pdf_url(req.version)
        };
        let bytes = net::get_bytes(&self.client, &self.rl, &url).await?;
        Ok(Some(bytes.to_vec()))
    }
}

// FUTURE: `S3RequesterPaysSource`.
//
// A requester-pays S3 bucket is planned. It will plug in here as another
// `PdfSource` (`is_network() == true`), inserted in `build_sources` *ahead* of
// `EprintHttpSource` and after `DownloadsSource`. Open question that shapes its
// `fetch`: whether the bucket is keyed by arbitrary version ids or only carries
// the current PDF. If current-only, it returns `Ok(None)` for `!req.is_current`
// exactly like `DownloadsSource`; if it supports versions, it keys the object
// on `req.version`. Either way the rest of the pipeline is unchanged.

/// Build the ordered source list for this run. Order = priority.
pub fn build_sources(cx: &Context) -> Vec<Box<dyn PdfSource>> {
    let mut sources: Vec<Box<dyn PdfSource>> = Vec::new();
    sources.push(Box::new(DownloadsSource { dir: cx.cfg.downloads_dir.clone() }));
    // S3RequesterPaysSource will be inserted here.
    match net::client(cx.cfg.network.contact.as_deref()) {
        Ok(client) => sources.push(Box::new(EprintHttpSource { client, rl: cx.rate_limiter.clone() })),
        Err(e) => warn!(error = %e, "skipping eprint-http source: could not build HTTP client"),
    }
    sources
}

/// Acquire the PDF for `req` from the first source that has it.
///
/// Network sources are skipped under `--offline`. If no source produces bytes,
/// returns an error listing what was tried.
pub async fn acquire(cx: &Context, req: &PdfRequest<'_>) -> Result<Acquired> {
    let sources = build_sources(cx);
    let mut tried = Vec::new();

    for source in &sources {
        if cx.offline && source.is_network() {
            tried.push(format!("{} (skipped: --offline)", source.name()));
            continue;
        }
        match source.fetch(req).await {
            Ok(Some(bytes)) => {
                info!(source = source.name(), bytes = bytes.len(), "acquired PDF");
                return Ok(Acquired {
                    source: source.name(),
                    network: source.is_network(),
                    bytes,
                });
            }
            Ok(None) => tried.push(format!("{} (not available)", source.name())),
            Err(e) => {
                warn!(source = source.name(), error = %e, "source failed");
                tried.push(format!("{}: {e}", source.name()));
            }
        }
    }

    anyhow::bail!(
        "no source had {} version {}. Tried: [{}]. \
         (eprint PDFs are normally delivered to the downloads dir {} by the \
         companion watcher — is it running and the paper downloaded in a browser?)",
        req.id,
        req.version,
        tried.join("; "),
        cx.cfg.downloads_dir.display(),
    )
}
