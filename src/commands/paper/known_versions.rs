//! Refresh a paper's version list from the archive listing
//! (`/archive/versions/<id>`), the authoritative source of versions.

use crate::cache::{self, PaperMeta};
use crate::cli::Context;
use crate::iacr::archive;
use crate::iacr::http;
use crate::ids::PaperId;
use anyhow::Result;

/// Fetch the archive listing for `id`, record it in the paper's meta
/// (creating one if needed; title and sync hints are preserved), and persist.
pub async fn refresh(cx: &Context, id: PaperId, existing: Option<PaperMeta>) -> Result<PaperMeta> {
    let client = http::client(cx.cfg.network.contact.as_deref())?;
    let versions = archive::fetch_versions(&client, &cx.rate_limiter, &id.archive_url()).await?;
    let current = versions.iter().find(|v| v.is_current).map(|v| v.timestamp);
    let mut meta = existing.unwrap_or_else(|| PaperMeta::new(None, Vec::new()));
    meta.record_listing(versions.iter().map(|v| v.timestamp).collect(), current);
    cache::write_paper_meta(&cx.cfg.cache_root, id, &meta).await?;
    Ok(meta)
}
