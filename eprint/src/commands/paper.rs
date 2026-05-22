//! `eprint <id>` — the universal "describe + acquire" entrypoint.
//!
//! Default behavior: ensure the paper's current version is fetched
//! (PDF + bib + abstract + version list), then print a summary of what
//! we know about it.
//!
//! Flags:
//! - `--md[=text|ml]` also produces Markdown for the resolved version
//! - `--version <ts>` operates on a specific historical version
//! - `--select-version` opens an interactive picker (dialoguer)
//! - `--force` ignores staleness and hits the network
//! - global `--offline` keeps everything from going to the network

use crate::archive::{self, ArchiveVersion};
use crate::cache::{self, PaperMeta, VersionMeta};
use crate::cli::{Context, PaperArgs};
use crate::id::PaperId;
use crate::net::{self, RateLimiter};
use crate::scrape;
use crate::version;
use anyhow::{Context as _, Result};
use papermd::{Converter, LocalConverter, Quality, RemoteConverter};
use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

#[derive(Debug, Serialize)]
pub struct PaperReport {
    pub id: String,
    pub title: Option<String>,
    pub current_version: Option<String>,
    pub resolved_version: Option<String>,
    pub directory: Option<String>,
    pub known_versions: Vec<String>,
    pub cached_versions: Vec<String>,
    pub md_quality: Option<String>,
    pub bytes_downloaded: u64,
    pub actions: Vec<&'static str>,
}

pub async fn run(cx: &Context, args: PaperArgs) -> Result<()> {
    let id: PaperId = args.id.parse().context("parsing paper id")?;
    crate::commands::sync::maybe_auto_sync(cx).await?;

    let mut report = PaperReport {
        id: id.canonical(),
        title: None,
        current_version: None,
        resolved_version: None,
        directory: None,
        known_versions: Vec::new(),
        cached_versions: Vec::new(),
        md_quality: None,
        bytes_downloaded: 0,
        actions: Vec::new(),
    };

    let root = &cx.cfg.cache_root;
    let mut paper_meta = cache::read_paper_meta(root, id).await;

    // 1. Make sure we know which versions exist on eprint (scrape archive
    //    if --force or if we have nothing on file).
    let need_archive = args.force || paper_meta.as_ref().map(|p| p.known_versions.is_empty()).unwrap_or(true);
    if need_archive && !cx.offline {
        match fetch_archive_list(cx, id).await {
            Ok((versions, bytes)) => {
                report.bytes_downloaded += bytes;
                let canonical_list: Vec<String> = versions.iter().map(|v| v.timestamp.clone()).collect();
                let current = versions.iter().find(|v| v.is_current).map(|v| v.timestamp.clone());
                let pm = paper_meta.get_or_insert_with(|| {
                    // Default: assume the current marker is correct.
                    let cv = current.clone().unwrap_or_else(|| {
                        canonical_list.last().cloned().unwrap_or_default()
                    });
                    PaperMeta {
                        tool: cache::TOOL_TAG.into(),
                        current_version: Some(cv),
                        known_versions: canonical_list.clone(),
                        title: None,
                    }
                });
                pm.known_versions = canonical_list;
                if let Some(c) = current {
                    pm.current_version = Some(c);
                }
                cache::write_paper_meta(root, id, pm).await?;
                report.actions.push("archive-listed");
            }
            Err(e) => warn!(error = %e, "could not scrape archive listing; falling back to whatever's on file"),
        }
    }

    // 2. Resolve which version we're operating on.
    let target_version = resolve_target_version(cx, id, paper_meta.as_ref(), &args).await?;

    // 3. Ensure that version's PDF is on disk.
    let resolved_version = match target_version {
        Some(v) => {
            ensure_version_fetched(cx, id, &v, paper_meta.as_mut(), &mut report).await?;
            Some(v)
        }
        None => {
            // No version known and we couldn't (or weren't allowed to) scrape.
            None
        }
    };

    // 4. Reload meta in case it was written during fetch.
    let paper_meta = cache::read_paper_meta(root, id).await;
    if let Some(pm) = &paper_meta {
        report.title = pm.title.clone();
        report.current_version = pm.current_version.clone();
        report.known_versions = pm.known_versions.clone();
    }
    report.cached_versions = cache::existing_versions(root, id);
    report.resolved_version = resolved_version.clone();
    if let Some(v) = &resolved_version {
        report.directory = Some(cache::version_dir(root, id, v).display().to_string());
    }

    // 5. Optional markdown conversion.
    if let Some(md_quality) = args.md_quality() {
        if let Some(v) = &resolved_version {
            let vmeta = cache::read_version_meta(root, id, v).await;
            let already_ok = paper_md_quality(&vmeta)
                .map(|q| quality_at_least(q, md_quality))
                .unwrap_or(false);
            if !already_ok {
                run_convert(cx, id, v, md_quality, &mut report).await?;
            } else {
                info!(id = %id, version = %v, quality = ?md_quality, "markdown already cached at requested quality");
            }
            let vmeta = cache::read_version_meta(root, id, v).await;
            report.md_quality = vmeta.md_quality;
        } else {
            anyhow::bail!("can't produce markdown: no version resolved");
        }
    } else if let Some(v) = &resolved_version {
        let vmeta = cache::read_version_meta(root, id, v).await;
        report.md_quality = vmeta.md_quality;
    }

    // 6. Emit.
    emit(cx, &args, &report).await?;
    Ok(())
}

/// Decide which version timestamp the user wants. Priority:
/// 1. `--version <ts>` if given
/// 2. `--select-version` -> dialoguer picker over known versions
/// 3. paper_meta.current_version
async fn resolve_target_version(
    cx: &Context,
    id: PaperId,
    paper_meta: Option<&PaperMeta>,
    args: &PaperArgs,
) -> Result<Option<String>> {
    if let Some(v) = &args.version {
        anyhow::ensure!(
            version::is_canonical(v),
            "--version expects canonical timestamp YYYYMMDDTHHMMSSZ, got {v:?}"
        );
        return Ok(Some(v.clone()));
    }
    if args.select_version {
        anyhow::ensure!(!cx.json, "--select-version is interactive; incompatible with --json");
        let versions: Vec<String> = paper_meta
            .map(|p| p.known_versions.clone())
            .unwrap_or_default();
        anyhow::ensure!(
            !versions.is_empty(),
            "no known versions to choose from (run without --offline to scrape the archive)"
        );
        let labels: Vec<String> = versions
            .iter()
            .map(|v| {
                let current_marker = paper_meta
                    .and_then(|p| p.current_version.as_deref())
                    .map(|c| c == v)
                    .unwrap_or(false);
                let cached_marker = cache::version_dir(&cx.cfg.cache_root, id, v).exists();
                let mut tags = Vec::new();
                if current_marker { tags.push("current"); }
                if cached_marker { tags.push("cached"); }
                if tags.is_empty() {
                    v.clone()
                } else {
                    format!("{v}   ({})", tags.join(", "))
                }
            })
            .collect();
        let idx = dialoguer::Select::new()
            .with_prompt("Pick a version")
            .items(&labels)
            .default(labels.len() - 1)
            .interact()?;
        return Ok(Some(versions[idx].clone()));
    }
    Ok(paper_meta.and_then(|p| p.current_version.clone()))
}

async fn fetch_archive_list(cx: &Context, id: PaperId) -> Result<(Vec<ArchiveVersion>, u64)> {
    let client = net::client(cx.cfg.network.contact.as_deref())?;
    let rl = RateLimiter::new(net::rate_limit_path(&cx.cfg.cache_root), cx.cfg.network.min_interval_s);
    let versions = archive::fetch_versions(&client, &rl, &id.archive_url()).await?;
    // We don't separately track bytes from the archive scrape; rough estimate
    // is negligible (a few KB of HTML), so report 0 here and let download bytes
    // dominate the report.
    Ok((versions, 0))
}

async fn ensure_version_fetched(
    cx: &Context,
    id: PaperId,
    version: &str,
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
    let rl = RateLimiter::new(net::rate_limit_path(root), cx.cfg.network.min_interval_s);

    // Always fetch from the canonical /YYYY/NNN.pdf URL — eprint serves
    // the current version there. For historical versions we'd need to
    // hit /archive/<year>/<num>/<unix>.pdf, but we don't currently
    // record the unix seconds; punt on historical PDF fetching for now
    // and surface a clear error.
    let is_current = paper_meta
        .as_deref()
        .and_then(|p| p.current_version.as_deref())
        == Some(version);
    if !is_current {
        anyhow::bail!(
            "fetching non-current versions isn't implemented yet \
             (would need to record per-version unix-seconds PDF URL); \
             current version {} is what eprint serves at /{}/{}.pdf",
            version,
            id.year,
            id.num
        );
    }

    let pdf_bytes = net::get_bytes(&client, &rl, &id.pdf_url()).await?;
    anyhow::ensure!(
        net::looks_like_pdf(&pdf_bytes),
        "downloaded {} bytes that don't look like a PDF",
        pdf_bytes.len()
    );
    tokio::fs::write(&paths.pdf, &pdf_bytes).await?;
    report.bytes_downloaded += pdf_bytes.len() as u64;

    // Landing page for abstract + bibtex + title.
    let html = net::get_text(&client, &rl, &id.html_url()).await?;
    report.bytes_downloaded += html.len() as u64;
    let landing = scrape::parse(&html);

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

    // Per-version meta.
    let vmeta = VersionMeta {
        fetched_unix_s: Some(now_unix()),
        md_quality: None,
        mineru_version: None,
    };
    cache::write_version_meta(root, id, version, &vmeta).await?;

    // Update paper-level title.
    if let Some(pm) = paper_meta {
        if landing.title.is_some() {
            pm.title = landing.title.clone();
        }
        cache::write_paper_meta(root, id, pm).await?;
    } else {
        // No meta existed before; construct one from what we know.
        let mut pm = PaperMeta::for_first_fetch(version);
        pm.title = landing.title.clone();
        cache::write_paper_meta(root, id, &pm).await?;
    }

    report.actions.push("fetched-pdf");
    Ok(())
}

async fn run_convert(
    cx: &Context,
    id: PaperId,
    version: &str,
    quality: Quality,
    report: &mut PaperReport,
) -> Result<()> {
    let root = &cx.cfg.cache_root;
    let paths = cache::version_paths(root, id, version);
    let markdown = match quality {
        Quality::Text => {
            let pdf_path = paths.pdf.clone();
            tokio::task::spawn_blocking(move || pdf_extract::extract_text(&pdf_path))
                .await
                .context("pdf-extract task panicked")?
                .context("pdf-extract failed")?
        }
        Quality::Ml => {
            use crate::config::BackendKind;
            let cfg = &cx.cfg.ml;
            let conv: Box<dyn Converter> = match cfg.kind {
                BackendKind::Local => Box::new(LocalConverter::default()),
                BackendKind::Remote => {
                    let endpoint = cfg.endpoint.as_deref().ok_or_else(|| {
                        anyhow::anyhow!("EPRINT_ML_BACKEND=remote requires EPRINT_ML_ENDPOINT")
                    })?;
                    let token = cfg.token_env.as_deref().and_then(|v| std::env::var(v).ok());
                    let mut rc = RemoteConverter::new(endpoint)?;
                    if let Some(t) = token { rc = rc.with_token(t); }
                    Box::new(rc)
                }
            };
            let conv_result = conv.convert(&paths.pdf, Quality::Ml).await?;
            conv_result.markdown
        }
    };
    tokio::fs::write(&paths.md, &markdown).await?;
    let mut vmeta = cache::read_version_meta(root, id, version).await;
    vmeta.md_quality = Some(match quality { Quality::Text => "text".into(), Quality::Ml => "ml".into() });
    if quality == Quality::Ml {
        vmeta.mineru_version = Some(papermd::local::MINERU_VERSION.to_owned());
    }
    cache::write_version_meta(root, id, version, &vmeta).await?;
    report.actions.push(if matches!(quality, Quality::Text) { "converted-text" } else { "converted-ml" });
    Ok(())
}

async fn emit(cx: &Context, args: &PaperArgs, report: &PaperReport) -> Result<()> {
    if cx.json {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    }
    println!("{}", report.id);
    if let Some(t) = &report.title {
        println!("  title: {t}");
    }
    if let Some(v) = &report.current_version {
        println!("  current version: {v}");
    }
    if let Some(v) = &report.resolved_version {
        if Some(v) != report.current_version.as_ref() {
            println!("  resolved to:     {v}");
        }
    }
    if !report.known_versions.is_empty() {
        let total = report.known_versions.len();
        let cached = report.cached_versions.len();
        println!("  versions: {total} known, {cached} cached");
        for v in report.known_versions.iter().rev() {
            let mut tags = Vec::new();
            if Some(v) == report.current_version.as_ref() { tags.push("current"); }
            if report.cached_versions.contains(v) { tags.push("cached"); }
            let tag_str = if tags.is_empty() { String::new() } else { format!("   ({})", tags.join(", ")) };
            println!("{v}{tag_str}");
        }
    }
    if let Some(q) = &report.md_quality {
        println!("  markdown:        {q}");
    }
    if !report.actions.is_empty() {
        println!("  did:             {}", report.actions.join(", "));
    }
    if report.bytes_downloaded > 0 {
        println!("  bytes:           {}", report.bytes_downloaded);
    }
    // Abstract printed last (best-effort).
    if !args.no_abstract {
        if let Some(v) = &report.resolved_version {
            let abstract_path = cache::version_paths(&cx.cfg.cache_root, args.id.parse().unwrap(), v).abstract_;
            if let Ok(abs) = tokio::fs::read_to_string(&abstract_path).await {
                println!();
                println!("Abstract:");
                for line in abs.lines() {
                    println!("  {line}");
                }
            }
        }
    }
    Ok(())
}

fn now_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn paper_md_quality(vmeta: &VersionMeta) -> Option<Quality> {
    match vmeta.md_quality.as_deref()? {
        "text" => Some(Quality::Text),
        "ml" => Some(Quality::Ml),
        _ => None,
    }
}

fn quality_at_least(cached: Quality, requested: Quality) -> bool {
    matches!((cached, requested),
        (Quality::Ml, _) | (Quality::Text, Quality::Text)
    )
}

