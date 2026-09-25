//! Pluggable PDF byte sources.
//!
//! Today a paper's PDF comes from eprint.iacr.org over HTTP. A
//! **requester-pays S3 bucket** is planned as another source.
//!
//! Rather than special-casing each, acquisition goes through an ordered list of
//! [`PdfSource`]s. [`acquire`] tries each in turn and takes the first that
//! yields bytes. Adding S3 later is a matter of writing one more `PdfSource`
//! and inserting it into [`build_sources`] — no churn in the fetch path.
//!
//! Metadata (title/abstract/bibtex via the landing page, version datestamps via
//! OAI-PMH) is a separate concern handled by the caller; sources only produce
//! PDF bytes.

mod eprint_http;

use crate::cli::Context;
use crate::iacr::http;
use crate::ids::version::Canonical;
use crate::ids::PaperId;
use anyhow::Result;
use async_trait::async_trait;
use eprint_http::EprintHttpSource;
use tracing::{info, warn};

/// What the caller wants: a specific version of a paper's PDF.
pub struct PdfRequest<'a> {
    pub id: PaperId,
    pub version: &'a Canonical,
    /// True iff `version` is the paper's current version. eprint serves the
    /// current and historical PDFs from different URLs; the S3 bucket may only
    /// hold the current one.
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

// FUTURE: `S3RequesterPaysSource`.
//
// A requester-pays S3 bucket is planned. It will be another `PdfSource` in
// its own module (`is_network() == true`), inserted in `build_sources` *ahead* of
// `EprintHttpSource`. Open question that shapes its `fetch`: whether the bucket
// is keyed by arbitrary version ids or only carries the current PDF. If
// current-only, it returns `Ok(None)` for `!req.is_current`; if it supports
// versions, it keys the object on `req.version`. Either way the rest of the
// pipeline is unchanged.

/// Build the ordered source list for this run. Order = priority.
pub fn build_sources(cx: &Context) -> Vec<Box<dyn PdfSource>> {
    let mut sources: Vec<Box<dyn PdfSource>> = Vec::new();
    // S3RequesterPaysSource will be inserted here.
    match http::client(cx.cfg.network.contact.as_deref()) {
        Ok(client) => sources.push(Box::new(EprintHttpSource::new(
            client,
            cx.rate_limiter.clone(),
        ))),
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

    Err(crate::exit::CommandFailure::PdfUnavailable(unavailable_message(cx, req, &tried)).into())
}

/// Build the actionable error for when no source produced the PDF. The
/// per-source reasons (e.g. the HTTP error) are already in `tried`; this adds
/// what to do next.
fn unavailable_message(cx: &Context, req: &PdfRequest<'_>, tried: &[String]) -> String {
    use std::fmt::Write as _;
    let mut m = format!(
        "could not acquire a PDF for {} (version {}).\nSources tried: {}.\n\n",
        req.id,
        req.version,
        tried.join("; "),
    );
    if cx.offline {
        let _ = write!(
            m,
            "This version's PDF isn't in the cache and --offline skips network sources. \
             Re-run without --offline to fetch it."
        );
    } else {
        let url = if req.is_current {
            req.id.pdf_url()
        } else {
            req.id.historical_pdf_url(req.version)
        };
        let _ = write!(
            m,
            "Fetching {url} failed (reason above). If eprint.iacr.org was rate-limiting, wait a \
             minute and re-run."
        );
    }
    m
}
