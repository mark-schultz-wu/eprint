//! `eprint fetch <id>` — download PDF + scrape abstract + BibTeX from
//! eprint.iacr.org into the local cache. Idempotent: existing artifacts
//! are kept (use `refresh` to overwrite).

use crate::cache::{self, Meta};
use crate::cli::{Context, FetchArgs};
use crate::id::PaperId;
use crate::net::{self, RateLimiter};
use crate::scrape;
use anyhow::{Context as _, Result};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

/// Result of one `fetch` invocation, summarized for JSON output / logging.
#[derive(Debug, serde::Serialize)]
pub struct FetchReport {
    pub id: String,
    pub directory: String,
    pub pdf_path: String,
    pub bib_path: String,
    pub abstract_path: String,
    pub bytes_downloaded: u64,
    pub cache_hits: Vec<&'static str>,
    pub fetched: Vec<&'static str>,
    pub title: Option<String>,
}

pub async fn run(cx: &Context, args: FetchArgs) -> Result<()> {
    let id: PaperId = args.id.parse().context("parsing paper id")?;
    let report = fetch_all(cx, id).await?;
    if cx.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        human_summary(&report);
    }
    Ok(())
}

pub async fn fetch_all(cx: &Context, id: PaperId) -> Result<FetchReport> {
    let root = &cx.cfg.cache_root;
    let paths = cache::paths_for(root, id);
    tokio::fs::create_dir_all(&paths.dir).await?;

    let client = net::client(cx.cfg.network.contact.as_deref())?;
    let rl = RateLimiter::new(net::rate_limit_path(root), cx.cfg.network.min_interval_s);

    let mut report = FetchReport {
        id: id.canonical(),
        directory: paths.dir.display().to_string(),
        pdf_path: paths.pdf.display().to_string(),
        bib_path: paths.bib.display().to_string(),
        abstract_path: paths.abstract_.display().to_string(),
        bytes_downloaded: 0,
        cache_hits: Vec::new(),
        fetched: Vec::new(),
        title: None,
    };

    // PDF
    if paths.pdf.exists() && paths.pdf.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        report.cache_hits.push("pdf");
    } else {
        ensure_can_network(cx, "pdf")?;
        let bytes = net::get_bytes(&client, &rl, &id.pdf_url()).await?;
        anyhow::ensure!(net::looks_like_pdf(&bytes), "downloaded {} bytes that don't look like a PDF (for {})", bytes.len(), id.pdf_url());
        tokio::fs::write(&paths.pdf, &bytes).await?;
        report.bytes_downloaded += bytes.len() as u64;
        report.fetched.push("pdf");
    }

    // Landing page (used to derive both abstract + bibtex + title).
    // We only fetch it if at least one of those is missing.
    let need_landing = !paths.bib.exists() || !paths.abstract_.exists();
    if need_landing {
        ensure_can_network(cx, "landing page (for abstract/bibtex)")?;
        let html = net::get_text(&client, &rl, &id.html_url()).await?;
        report.bytes_downloaded += html.len() as u64;
        let landing = scrape::parse(&html);

        if !paths.bib.exists() {
            match landing.bibtex.as_deref() {
                Some(bib) => {
                    tokio::fs::write(&paths.bib, bib).await?;
                    report.fetched.push("bib");
                }
                None => warn!("could not scrape BibTeX from landing page"),
            }
        } else {
            report.cache_hits.push("bib");
        }

        if !paths.abstract_.exists() {
            match landing.abstract_.as_deref() {
                Some(abs) => {
                    tokio::fs::write(&paths.abstract_, abs).await?;
                    report.fetched.push("abstract");
                }
                None => warn!("could not scrape abstract from landing page"),
            }
        } else {
            report.cache_hits.push("abstract");
        }

        report.title = landing.title;
    } else {
        report.cache_hits.push("bib");
        report.cache_hits.push("abstract");
    }

    update_meta(&paths.meta, &report).await?;
    info!(id = %id, hits = ?report.cache_hits, fetched = ?report.fetched, "fetch complete");
    Ok(report)
}

fn ensure_can_network(cx: &Context, what: &str) -> Result<()> {
    if cx.offline {
        anyhow::bail!("--offline set; cache miss for {what}");
    }
    Ok(())
}

async fn update_meta(meta_path: &Path, _report: &FetchReport) -> Result<()> {
    let mut meta: Meta = match tokio::fs::read_to_string(meta_path).await {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => Meta::default(),
    };
    meta.fetched_unix_s = Some(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
    );
    tokio::fs::write(meta_path, serde_json::to_vec_pretty(&meta)?).await?;
    Ok(())
}

fn human_summary(r: &FetchReport) {
    println!("{}", r.id);
    if let Some(t) = &r.title {
        println!("  title:    {t}");
    }
    println!("  dir:      {}", r.directory);
    if !r.fetched.is_empty() {
        println!("  fetched:  {}", r.fetched.join(", "));
    }
    if !r.cache_hits.is_empty() {
        println!("  cached:   {}", r.cache_hits.join(", "));
    }
    if r.bytes_downloaded > 0 {
        println!("  bytes:    {}", r.bytes_downloaded);
    }
}
