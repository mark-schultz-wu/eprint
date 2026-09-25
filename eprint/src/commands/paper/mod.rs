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

/// The result of an `eprint paper` run, ready to emit.
///
/// Built via [`ReportBuilder`]: `resolved_version` and `directory` are
/// non-`Option` because a `PaperReport` can only be produced once a version
/// has actually been resolved — [`ReportBuilder::resolve`] consumes the
/// version to make one. Resolution failure returns a coded error instead of a
/// report, so "no version" is unrepresentable here rather than a `None` every
/// consumer has to re-check.
#[derive(Debug, Serialize)]
pub struct PaperReport {
    pub id: String,
    pub title: Option<String>,
    pub current_version: Option<crate::version::Canonical>,
    pub resolved_version: crate::version::Canonical,
    pub directory: String,
    pub known_versions: Vec<crate::version::Canonical>,
    pub cached_versions: Vec<crate::version::Canonical>,
    pub md_quality: Option<String>,
    pub bytes_downloaded: u64,
    pub actions: Vec<&'static str>,
}

/// Accumulates the parts of a [`PaperReport`] that are known *before* a version
/// is resolved: the running `actions` log and the `bytes_downloaded` counter
/// that the archive/fetch/convert steps append to as they run. Calling
/// [`resolve`](Self::resolve) consumes the builder together with the resolved
/// version, yielding a [`PaperReport`] whose post-resolution metadata fields
/// are then filled in directly.
pub struct ReportBuilder {
    id: String,
    bytes_downloaded: u64,
    actions: Vec<&'static str>,
}

impl ReportBuilder {
    pub fn new(id: String) -> Self {
        Self { id, bytes_downloaded: 0, actions: Vec::new() }
    }

    /// Record a step that was performed (shown in the report's `did:` line).
    pub fn action(&mut self, action: &'static str) {
        self.actions.push(action);
    }

    /// Add to the network-bytes counter.
    pub fn add_downloaded(&mut self, bytes: u64) {
        self.bytes_downloaded += bytes;
    }

    /// Finish into a [`PaperReport`] now that a version is resolved. The caller
    /// can't reach this without a `Canonical`, which is exactly why the
    /// report's `resolved_version`/`directory` are non-`Option`. The remaining
    /// metadata fields (title, versions, md_quality) default to empty and are
    /// set by the caller afterwards.
    pub fn resolve(self, version: crate::version::Canonical, directory: String) -> PaperReport {
        PaperReport {
            id: self.id,
            title: None,
            current_version: None,
            resolved_version: version,
            directory,
            known_versions: Vec::new(),
            cached_versions: Vec::new(),
            md_quality: None,
            bytes_downloaded: self.bytes_downloaded,
            actions: self.actions,
        }
    }
}

pub async fn run(cx: &Context, args: PaperArgs) -> Result<()> {
    let id: PaperId = args.id.parse().context("parsing paper id")?;
    crate::commands::sync::maybe_auto_sync(cx).await?;

    let mut report = ReportBuilder::new(id.canonical());

    // 1. Refresh the archive listing if needed.
    let root = &cx.cfg.cache_root;
    let mut paper_meta = cache::read_paper_meta(root, id).await;
    let need_archive = args.force || paper_meta.as_ref().map(|p| p.known_versions.is_empty()).unwrap_or(true);
    if need_archive && !cx.offline {
        match archive::refresh_known_versions(cx, id, paper_meta.clone()).await {
            Ok(new_meta) => {
                paper_meta = Some(new_meta);
                report.action("archive-listed");
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
                    report.action("oai-resolved");
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
    let Some(v) = target_version else {
        // No version to operate on: archive scrape was blocked (Cloudflare 403)
        // AND the OAI fallback didn't yield a current version. The PDF source
        // list is never consulted, so any local PDF in the downloads dir goes
        // unused — not because it's missing, but because there's no version to
        // file it under. This used to fall through to an (almost empty) report
        // and exit 0, masking the failure; make it a hard, coded error instead.
        let downloads_pdf = crate::downloads::expected_pdf_path(&cx.cfg.downloads_dir, id);
        warn!(id = %id, offline = cx.offline, downloads_pdf = %downloads_pdf.display(), "no version resolved");
        return Err(crate::exit::CommandFailure::NoVersionResolved(format!(
            "could not resolve a version for {id}: the archive listing is blocked and the OAI \
             fallback returned nothing, so there's no version to file the PDF under. A local \
             downloaded PDF (if any) at {} can't be used without a version. Re-run online so \
             OAI-PMH can resolve the current version{}.",
            downloads_pdf.display(),
            if cx.offline { " (you are --offline)" } else { "" },
        ))
        .into());
    };
    // From here on a version is resolved unconditionally (the let-else above
    // returns otherwise), so it's a plain `Canonical`. The fetch and convert
    // steps only append to the running log, so they keep taking the builder;
    // the report itself isn't materialized until resolution is final.
    let version = v;
    fetch_one::ensure_version(cx, id, &version, paper_meta.as_mut(), &mut report).await?;

    // 3b. Persist the OAI abstract if the (often-blocked) landing scrape didn't
    //     already write one for this version.
    if let Some(abs) = &oai_abstract {
        let ap = cache::version_paths(root, id, &version).abstract_;
        if !ap.exists() {
            if let Err(e) = tokio::fs::write(&ap, abs).await {
                warn!(error = %e, "could not write OAI abstract");
            }
        }
    }

    // 4. Optional markdown conversion (still just appends to the running log).
    if let Some(quality) = args.md_quality() {
        convert::maybe_run(cx, id, &version, quality, &mut report).await?;
    }

    // 5. Finalize: a version is resolved, so build the report and fill in the
    //    post-resolution metadata. Reload meta first (fetch may have rewritten it).
    let directory = cache::version_dir(root, id, &version).display().to_string();
    let mut report = report.resolve(version, directory);
    let paper_meta = cache::read_paper_meta(root, id).await;
    if let Some(pm) = &paper_meta {
        report.title = pm.title.clone();
        report.current_version = pm.current_version;
        report.known_versions = pm.known_versions.clone();
    }
    report.cached_versions = cache::existing_versions(root, id);
    report.md_quality = cache::read_version_meta(root, id, &version).await.md_quality;

    emit::print(cx, &args, &report).await?;
    Ok(())
}
