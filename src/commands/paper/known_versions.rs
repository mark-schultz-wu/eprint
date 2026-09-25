//! Archive-page handling: scrape `/archive/versions/<id>` and update
//! `PaperMeta.known_versions` / `current_version`. Returns the updated
//! meta on success; errors propagate (caller can demote to warn).

use crate::cache::{self, PaperMeta, TOOL_TAG};
use crate::cli::Context;
use crate::iacr::archive;
use crate::iacr::http;
use crate::ids::PaperId;
use anyhow::Result;

/// Fetch the archive listing for `id` and merge into a fresh `PaperMeta`.
/// If `existing` is `Some`, preserves its `title` and any extra fields.
pub async fn refresh(cx: &Context, id: PaperId, existing: Option<PaperMeta>) -> Result<PaperMeta> {
    let client = http::client(cx.cfg.network.contact.as_deref())?;
    let versions = archive::fetch_versions(&client, &cx.rate_limiter, &id.archive_url()).await?;
    let canonical_list: Vec<crate::ids::version::Canonical> =
        versions.iter().map(|v| v.timestamp).collect();
    let current = versions.iter().find(|v| v.is_current).map(|v| v.timestamp);

    let new_meta = match existing {
        Some(mut pm) => {
            pm.known_versions = canonical_list;
            if let Some(c) = current {
                pm.current_version = Some(c);
            }
            pm
        }
        None => {
            let cv = current.or_else(|| canonical_list.last().cloned());
            PaperMeta {
                tool: TOOL_TAG.into(),
                current_version: cv,
                known_versions: canonical_list,
                title: None,
            }
        }
    };
    cache::write_paper_meta(&cx.cfg.cache_root, id, &new_meta).await?;
    Ok(new_meta)
}
