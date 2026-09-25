//! Ensure a specific version of a paper's PDF is in the cache.
//!
//! PDF bytes are acquired through the pluggable source list in
//! [`crate::source`] (downloads dir, then network; S3 later). Metadata
//! (title/bib/abstract) is scraped from the landing page best-effort, and
//! per-version + paper-level meta are updated on success.

use crate::cache::{self, PaperMeta, VersionMeta};
use crate::cli::Context;
use crate::id::PaperId;
use crate::net;
use crate::scrape;
use crate::source;
use crate::version::Canonical;
use crate::commands::paper::ReportBuilder;
use anyhow::Result;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, warn};

/// Ensure `<root>/<id>/<version>/paper.pdf` exists. Acquires it if missing.
/// Updates per-version + paper-level meta on success.
pub async fn ensure_version(
    cx: &Context,
    id: PaperId,
    version: &Canonical,
    paper_meta: Option<&mut PaperMeta>,
    report: &mut ReportBuilder,
) -> Result<()> {
    let root = &cx.cfg.cache_root;
    let paths = cache::version_paths(root, id, version);
    if paths.pdf.exists() {
        return Ok(());
    }
    tokio::fs::create_dir_all(&paths.dir).await?;

    // Is this the paper's current version? Some sources (the downloads dir)
    // only ever hold the current PDF; historical versions come from elsewhere.
    let current = paper_meta.as_deref().and_then(|p| p.current_version.as_ref());
    let is_current = current == Some(version);
    debug!(
        id = %id,
        target_version = %version,
        current_version = ?current,
        is_current,
        "resolving PDF source eligibility (is_current gates the downloads source)"
    );

    // Pull the bytes from the first source that has them.
    let acquired = source::acquire(cx, &source::PdfRequest { id, version, is_current }).await?;
    anyhow::ensure!(
        net::looks_like_pdf(&acquired.bytes),
        "{} bytes for {} version {} (source: {}) don't look like a PDF (missing %PDF header)",
        acquired.bytes.len(),
        id,
        version,
        acquired.source,
    );
    tokio::fs::write(&paths.pdf, &acquired.bytes).await?;
    if acquired.network {
        report.add_downloaded(acquired.bytes.len() as u64);
    }
    report.action(match acquired.source {
        "downloads" => "pdf-from-downloads",
        _ if is_current => "fetched-pdf",
        _ => "fetched-historical-pdf",
    });

    // Scrape the landing page for metadata when:
    //   * we're on the current version (its canonical bib/abstract live there), OR
    //   * we don't yet have a title on file (any landing visit yields one).
    //
    // The landing page always describes the *current* version, so for historical
    // fetches we keep the title (shared across versions) but not bib/abstract.
    //
    // NOTE: eprint.iacr.org landing pages are currently Cloudflare-blocked (403),
    // so this is best-effort: both fetch and parse failures demote to a warning
    // rather than failing the command — the PDF is already cached.
    let have_title = paper_meta
        .as_deref()
        .and_then(|p| p.title.as_deref())
        .is_some();
    let need_landing = (is_current || !have_title) && !cx.offline;
    if need_landing {
        let client = net::client(cx.cfg.network.contact.as_deref())?;
        let rl = &*cx.rate_limiter;
        let landing = match net::get_text(&client, rl, &id.html_url()).await {
            Ok(html) => {
                report.add_downloaded(html.len() as u64);
                scrape::parse(&html).unwrap_or_else(|e| {
                    warn!(error = %e, "could not parse landing page; continuing without title/bib/abstract");
                    scrape::Landing::default()
                })
            }
            Err(e) => {
                warn!(error = %e, "could not fetch landing page; continuing without title/bib/abstract");
                scrape::Landing::default()
            }
        };

        if is_current {
            if let Some(bib) = &landing.bibtex {
                tokio::fs::write(&paths.bib, bib).await?;
            }
            if let Some(abs) = &landing.abstract_ {
                tokio::fs::write(&paths.abstract_, abs).await?;
            }
        }

        // Title goes in paper_meta regardless of version (it's shared).
        if let Some(pm) = paper_meta {
            if landing.title.is_some() {
                pm.title = landing.title.clone();
            }
            cache::write_paper_meta(root, id, pm).await?;
        } else {
            let mut pm = PaperMeta::for_first_fetch(*version);
            pm.title = landing.title.clone();
            cache::write_paper_meta(root, id, &pm).await?;
        }
    }

    let vmeta = VersionMeta {
        fetched_unix_s: Some(now_unix()),
        md_quality: None,
        mineru_version: None,
    };
    cache::write_version_meta(root, id, version, &vmeta).await?;
    Ok(())
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
