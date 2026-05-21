//! `eprint fetch <ref>` — bring a paper's artifacts into the cache.
//!
//! Logic:
//! - `eprint fetch 2024/463` (no version pin):
//!   - If the paper has no cached versions → download into `v1/`.
//!   - If cache exists and is not marked stale → noop, point at current.
//!   - If cache exists and `paper.latest_known_oai_datestamp` is newer
//!     than the current version's `oai_datestamp` → download into a new
//!     `v{current+1}/`.
//! - `eprint fetch 2024/463@vN`:
//!   - Always serve `vN` if it exists. Error if it doesn't.
//!   - Never re-checks staleness.
//!
//! Sync (separate command) is what populates the OAI datestamps that drive
//! the staleness check. `fetch` alone never fills `oai_datestamp` — first-
//! time fetched versions get a `null` datestamp and rely on a subsequent
//! sync to backfill it.

use crate::cache::{self, PaperMeta, VersionMeta};
use crate::cli::{Context, FetchArgs};
use crate::id::{PaperId, PaperRef};
use crate::net::{self, RateLimiter};
use crate::scrape;
use anyhow::{Context as _, Result};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

#[derive(Debug, serde::Serialize)]
pub struct FetchReport {
    pub id: String,
    pub version: u32,
    pub directory: String,
    pub bytes_downloaded: u64,
    pub action: &'static str, // "cache-hit" | "first-fetch" | "new-version" | "version-pinned"
    pub title: Option<String>,
}

pub async fn run(cx: &Context, args: FetchArgs) -> Result<()> {
    let r: PaperRef = args.id.parse().context("parsing paper reference")?;
    let report = fetch_ref(cx, r).await?;
    if cx.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        human_summary(&report);
    }
    Ok(())
}

/// Resolve a `PaperRef` to a concrete cached version, fetching from the
/// network as needed.
pub async fn fetch_ref(cx: &Context, r: PaperRef) -> Result<FetchReport> {
    let root = &cx.cfg.cache_root;

    match r.version {
        // Explicit version pin: must already be in cache.
        Some(v) => {
            let paths = cache::version_paths(root, r.id, v);
            anyhow::ensure!(
                paths.pdf.exists(),
                "{}@v{v} is not in the cache",
                r.id
            );
            Ok(FetchReport {
                id: r.id.canonical(),
                version: v,
                directory: paths.dir.display().to_string(),
                bytes_downloaded: 0,
                action: "version-pinned",
                title: None,
            })
        }
        // No pin: serve current, or fetch a new version if stale.
        None => fetch_current(cx, r.id).await,
    }
}

async fn fetch_current(cx: &Context, id: PaperId) -> Result<FetchReport> {
    let root = &cx.cfg.cache_root;
    let paper_meta = cache::read_paper_meta(root, id).await;

    // Decide what to do based on current cache state.
    let action = decide_action(root, id, &paper_meta).await;

    match action {
        Decision::CacheHit { version } => {
            let paths = cache::version_paths(root, id, version);
            Ok(FetchReport {
                id: id.canonical(),
                version,
                directory: paths.dir.display().to_string(),
                bytes_downloaded: 0,
                action: "cache-hit",
                title: paper_meta.title,
            })
        }
        Decision::Download { version, action } => {
            if cx.offline {
                anyhow::bail!("--offline set; would need to fetch {} v{version}", id);
            }
            do_fetch(cx, id, version, paper_meta, action).await
        }
    }
}

enum Decision {
    CacheHit { version: u32 },
    Download { version: u32, action: &'static str },
}

async fn decide_action(root: &Path, id: PaperId, paper_meta: &PaperMeta) -> Decision {
    let versions = cache::existing_versions(root, id);
    if versions.is_empty() {
        return Decision::Download { version: 1, action: "first-fetch" };
    }
    let current = paper_meta.current_version.unwrap_or_else(|| *versions.last().unwrap());
    // Stale if sync has seen a newer OAI datestamp than the current version's.
    let version_meta = cache::read_version_meta(root, id, current).await;
    let stale = match (
        paper_meta.latest_known_oai_datestamp.as_deref(),
        version_meta.oai_datestamp.as_deref(),
    ) {
        (Some(seen), Some(cached)) => seen > cached,
        (Some(_), None) => true, // current version has no datestamp yet; if sync saw any, treat as stale
        _ => false,              // no sync info at all; trust the cache
    };
    if stale {
        let next = current.saturating_add(1);
        Decision::Download { version: next, action: "new-version" }
    } else {
        Decision::CacheHit { version: current }
    }
}

async fn do_fetch(
    cx: &Context,
    id: PaperId,
    version: u32,
    mut paper_meta: PaperMeta,
    action: &'static str,
) -> Result<FetchReport> {
    let root = &cx.cfg.cache_root;
    let paths = cache::version_paths(root, id, version);
    tokio::fs::create_dir_all(&paths.dir).await?;

    let client = net::client(cx.cfg.network.contact.as_deref())?;
    let rl = RateLimiter::new(net::rate_limit_path(root), cx.cfg.network.min_interval_s);

    let mut bytes_downloaded = 0u64;

    // PDF
    let pdf_bytes = net::get_bytes(&client, &rl, &id.pdf_url()).await?;
    anyhow::ensure!(
        net::looks_like_pdf(&pdf_bytes),
        "downloaded {} bytes that don't look like a PDF (for {})",
        pdf_bytes.len(),
        id.pdf_url()
    );
    tokio::fs::write(&paths.pdf, &pdf_bytes).await?;
    bytes_downloaded += pdf_bytes.len() as u64;

    // Landing page (for abstract + bibtex + title).
    let html = net::get_text(&client, &rl, &id.html_url()).await?;
    bytes_downloaded += html.len() as u64;
    let landing = scrape::parse(&html);

    if let Some(bib) = &landing.bibtex {
        tokio::fs::write(&paths.bib, bib).await?;
    } else {
        warn!("could not scrape BibTeX from landing page");
    }
    if let Some(abs) = &landing.abstract_ {
        tokio::fs::write(&paths.abstract_, abs).await?;
    } else {
        warn!("could not scrape abstract from landing page");
    }

    // Per-version meta.
    let vmeta = VersionMeta {
        fetched_unix_s: Some(now_unix()),
        oai_datestamp: None, // sync will fill this
        md_quality: None,
        mineru_version: None,
    };
    cache::write_version_meta(root, id, version, &vmeta).await?;

    // Paper-level meta: bump current_version, refresh title.
    paper_meta.current_version = Some(version);
    if landing.title.is_some() {
        paper_meta.title = landing.title.clone();
    }
    cache::write_paper_meta(root, id, &paper_meta).await?;

    info!(id = %id, version, %action, "fetch complete");
    Ok(FetchReport {
        id: id.canonical(),
        version,
        directory: paths.dir.display().to_string(),
        bytes_downloaded,
        action,
        title: landing.title,
    })
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn human_summary(r: &FetchReport) {
    println!("{}", r.id);
    if let Some(t) = &r.title {
        println!("  title:    {t}");
    }
    println!("  version:  v{}", r.version);
    println!("  dir:      {}", r.directory);
    println!("  action:   {}", r.action);
    if r.bytes_downloaded > 0 {
        println!("  bytes:    {}", r.bytes_downloaded);
    }
}
