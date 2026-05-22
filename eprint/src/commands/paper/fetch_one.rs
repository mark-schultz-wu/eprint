//! Download a single specific version of a paper into the cache.
//! Handles both current (canonical /<id>.pdf URL) and historical
//! (`/archive/<id>/<unix>.pdf` URL) versions, computing the latter
//! via `PaperId::historical_pdf_url`.

use crate::cache::{self, PaperMeta, VersionMeta};
use crate::cli::Context;
use crate::id::PaperId;
use crate::net;
use crate::scrape;
use crate::version::Canonical;
use crate::commands::paper::PaperReport;
use anyhow::Result;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::warn;

/// Ensure `<root>/<id>/<version>/paper.pdf` exists. Downloads if missing.
/// Updates per-version + paper-level meta on a successful download.
pub async fn ensure_version(
    cx: &Context,
    id: PaperId,
    version: &Canonical,
    paper_meta: Option<&mut PaperMeta>,
    report: &mut PaperReport,
) -> Result<()> {
    let root = &cx.cfg.cache_root;
    let paths = cache::version_paths(root, id, version);
    if paths.pdf.exists() {
        return Ok(());
    }
    if cx.offline {
        anyhow::bail!("--offline set; {} version {} not in cache", id, version);
    }

    tokio::fs::create_dir_all(&paths.dir).await?;
    let client = net::client(cx.cfg.network.contact.as_deref())?;
    let rl = &*cx.rate_limiter;

    // Decide which URL to hit. The canonical /<id>.pdf serves the current
    // version; older versions live at /archive/<id>/<unix>.pdf.
    let is_current = paper_meta
        .as_deref()
        .and_then(|p| p.current_version.as_ref())
        == Some(version);
    let pdf_url = if is_current {
        id.pdf_url()
    } else {
        id.historical_pdf_url(version)
    };

    let pdf_bytes = net::get_bytes(&client, &rl, &pdf_url).await?;
    anyhow::ensure!(
        net::looks_like_pdf(&pdf_bytes),
        "downloaded {} bytes from {pdf_url} that don't look like a PDF",
        pdf_bytes.len()
    );
    tokio::fs::write(&paths.pdf, &pdf_bytes).await?;
    report.bytes_downloaded += pdf_bytes.len() as u64;

    // Scrape the landing page when:
    //   * We're fetching the current version (canonical bib/abstract for
    //     this version live there), OR
    //   * We don't yet have a title on file (any landing-page visit will
    //     yield one, even when we're pulling a historical PDF).
    //
    // The landing page at /<id> always describes the *current* version,
    // so for historical fetches we save the title (shared across versions)
    // but NOT the bib/abstract (which would mislabel the historical dir).
    let have_title = paper_meta
        .as_deref()
        .and_then(|p| p.title.as_deref())
        .is_some();
    let need_landing = is_current || !have_title;
    if need_landing {
        let html = net::get_text(&client, &rl, &id.html_url()).await?;
        report.bytes_downloaded += html.len() as u64;
        let landing = scrape::parse(&html);

        if is_current {
            if let Some(bib) = &landing.bibtex {
                tokio::fs::write(&paths.bib, bib).await?;
            } else {
                warn!("could not scrape BibTeX");
            }
            if let Some(abs) = &landing.abstract_ {
                tokio::fs::write(&paths.abstract_, abs).await?;
            } else {
                warn!("could not scrape abstract");
            }
        }

        // Title goes in paper_meta regardless of version (it's shared).
        if let Some(pm) = paper_meta {
            if landing.title.is_some() {
                pm.title = landing.title.clone();
            }
            cache::write_paper_meta(root, id, pm).await?;
        } else {
            let mut pm = PaperMeta::for_first_fetch(version.clone());
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

    report.actions.push(if is_current { "fetched-pdf" } else { "fetched-historical-pdf" });
    Ok(())
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
