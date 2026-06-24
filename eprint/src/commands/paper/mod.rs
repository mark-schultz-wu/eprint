//! `eprint paper <id>` — describe + acquire.
//!
//! Orchestrates submodules: [`archive`] (eprint version listing),
//! [`resolve`] (choose which version), [`fetch_one`] (download a
//! specific version), [`convert`] (run papermd on the PDF), [`emit`]
//! (format output).
//!
//! `run` is intentionally thin: it sequences the steps and threads a
//! [`PaperReport`] through. Each step is in its own module for
//! testability.

mod archive;
mod convert;
mod emit;
mod fetch_one;
mod resolve;

use crate::cache;
use crate::cli::{Context, PaperArgs};
use crate::id::PaperId;
use crate::net;
use crate::oai;
use crate::version;
use anyhow::{Context as _, Result};
use serde::Serialize;
use tracing::warn;

#[derive(Debug, Serialize)]
pub struct PaperReport {
    pub id: String,
    pub title: Option<String>,
    pub current_version: Option<crate::version::Canonical>,
    pub resolved_version: Option<crate::version::Canonical>,
    pub directory: Option<String>,
    pub known_versions: Vec<crate::version::Canonical>,
    pub cached_versions: Vec<crate::version::Canonical>,
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

    // 1. Refresh the archive listing if needed.
    let root = &cx.cfg.cache_root;
    let mut paper_meta = cache::read_paper_meta(root, id).await;
    let need_archive = args.force || paper_meta.as_ref().map(|p| p.known_versions.is_empty()).unwrap_or(true);
    if need_archive && !cx.offline {
        match archive::refresh_known_versions(cx, id, paper_meta.clone()).await {
            Ok(new_meta) => {
                paper_meta = Some(new_meta);
                report.actions.push("archive-listed");
            }
            Err(e) => warn!(error = %e, "could not scrape archive listing; falling back to whatever's on file"),
        }
    }

    // 1b. If we still don't know the current version (archive scrape blocked,
    //     or a brand-new paper), fall back to OAI-PMH GetRecord — the reliably
    //     reachable endpoint — for the current version plus title/abstract.
    //     Without this, a PDF already delivered to the downloads dir can't be
    //     filed, since there's no version to file it under.
    let mut oai_abstract: Option<String> = None;
    let have_current = paper_meta
        .as_ref()
        .and_then(|p| p.current_version.as_ref())
        .is_some();
    if !have_current && !cx.offline {
        let client = net::client(cx.cfg.network.contact.as_deref())?;
        match oai::get_record(&client, &cx.rate_limiter, id).await {
            Ok(Some(rec)) => match rec.datestamp.parse::<version::OaiDatestamp>() {
                Ok(ds) => {
                    let cv: version::Canonical = (&ds).into();
                    let mut pm = paper_meta
                        .take()
                        .unwrap_or_else(|| cache::PaperMeta::for_first_fetch(cv));
                    pm.current_version = Some(cv);
                    if !pm.known_versions.contains(&cv) {
                        pm.known_versions.push(cv);
                        pm.known_versions.sort();
                    }
                    if rec.title.is_some() {
                        pm.title = rec.title.clone();
                    }
                    cache::write_paper_meta(root, id, &pm).await?;
                    paper_meta = Some(pm);
                    oai_abstract = rec.abstract_;
                    report.actions.push("oai-resolved");
                }
                Err(e) => warn!(error = %e, "OAI datestamp not parseable; skipping OAI fallback"),
            },
            Ok(None) => warn!("OAI-PMH has no record for {id}"),
            Err(e) => warn!(error = %e, "OAI-PMH GetRecord failed"),
        }
    }

    // 2. Pick a version to operate on.
    let target_version = resolve::target_version(cx, id, paper_meta.as_ref(), &args).await?;

    // 3. Ensure that version's PDF is on disk.
    let resolved_version = if let Some(v) = target_version {
        fetch_one::ensure_version(cx, id, &v, paper_meta.as_mut(), &mut report).await?;
        Some(v)
    } else {
        None
    };

    // 3b. Persist the OAI abstract if the (often-blocked) landing scrape didn't
    //     already write one for this version.
    if let (Some(abs), Some(v)) = (&oai_abstract, &resolved_version) {
        let ap = cache::version_paths(root, id, v).abstract_;
        if !ap.exists() {
            if let Err(e) = tokio::fs::write(&ap, abs).await {
                warn!(error = %e, "could not write OAI abstract");
            }
        }
    }

    // 4. Reload meta (fetch may have rewritten it) for emit.
    let paper_meta = cache::read_paper_meta(root, id).await;
    if let Some(pm) = &paper_meta {
        report.title = pm.title.clone();
        report.current_version = pm.current_version;
        report.known_versions = pm.known_versions.clone();
    }
    report.cached_versions = cache::existing_versions(root, id);
    report.resolved_version = resolved_version;
    if let Some(v) = &resolved_version {
        report.directory = Some(cache::version_dir(root, id, v).display().to_string());
    }

    // 5. Optional markdown conversion.
    if let Some(quality) = args.md_quality() {
        if let Some(v) = &resolved_version {
            convert::maybe_run(cx, id, v, quality, &mut report).await?;
        } else {
            anyhow::bail!("can't produce markdown: no version resolved");
        }
    }
    if let Some(v) = &resolved_version {
        let vmeta = cache::read_version_meta(root, id, v).await;
        report.md_quality = vmeta.md_quality;
    }

    emit::print(cx, &args, &report).await?;
    Ok(())
}
