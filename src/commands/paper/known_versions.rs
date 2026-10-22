//! Refresh a paper's version list.
//!
//! On a host with the version archive (eprint.iacr.org), the listing at
//! `/archive/versions/<id>` is authoritative. On a host without it (the
//! default mirror), OAI-PMH `GetRecord` gives the paper's current
//! datestamp, which becomes the current version's label; versions seen on
//! earlier runs stay known.

use crate::cache::{self, PaperMeta};
use crate::cli::Context;
use crate::iacr::{archive, oai};
use crate::ids::version::{Canonical, OaiDatestamp};
use crate::ids::PaperId;
use anyhow::{Context as _, Result};

/// Refresh the version list for `id`, record it in the paper's meta (creating
/// one if needed; title and sync hints are preserved), and persist.
pub async fn refresh(cx: &Context, id: PaperId, existing: Option<PaperMeta>) -> Result<PaperMeta> {
    if !cx.site.has_archive() {
        return refresh_from_oai(cx, id, existing).await;
    }
    let versions =
        archive::fetch_versions(&cx.http, &cx.rate_limiter, &cx.site.archive_url(id)).await?;
    let current = versions.iter().find(|v| v.is_current).map(|v| v.timestamp);
    let mut meta = existing.unwrap_or_else(|| PaperMeta::new(None, Vec::new()));
    meta.record_listing(versions.iter().map(|v| v.timestamp).collect(), current);
    cache::write_paper_meta(&cx.cfg.cache_root, id, &meta).await?;
    Ok(meta)
}

/// No archive: the current version is the paper's OAI-PMH datestamp.
///
/// That datestamp is the paper's last modification, which may be a
/// metadata-only edit after the newest PDF. Labelling the current PDF with
/// it is still safe: the PDF fetched next is the one current *now*, so a
/// label can only run ahead of the PDF's true upload time, never behind it
/// (which is what filed a newer PDF under an older version in schema 0).
async fn refresh_from_oai(
    cx: &Context,
    id: PaperId,
    existing: Option<PaperMeta>,
) -> Result<PaperMeta> {
    let rec = oai::get_record(&cx.http, &cx.rate_limiter, &cx.site.oai_url(), id)
        .await?
        .with_context(|| format!("OAI-PMH has no record for {id}"))?;
    let ds: OaiDatestamp = rec
        .datestamp
        .parse()
        .with_context(|| format!("OAI datestamp {:?} is not parseable", rec.datestamp))?;
    let current: Canonical = (&ds).into();
    let mut meta = existing.unwrap_or_else(|| PaperMeta::new(None, Vec::new()));
    meta.record_current(current);
    if rec.title.is_some() {
        meta.title = rec.title;
    }
    cache::write_paper_meta(&cx.cfg.cache_root, id, &meta).await?;
    Ok(meta)
}
