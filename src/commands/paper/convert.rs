//! Produce `paper.md` for one cached version (see [`crate::markdown`]).
//!
//! Cached Markdown is reused only if the current converter produced it;
//! anything older (e.g. from the retired MinerU 3 / pdf-extract tiers) is
//! regenerated.

use crate::cache;
use crate::cli::Context;
use crate::commands::paper::ReportBuilder;
use crate::ids::version::Canonical;
use crate::ids::PaperId;
use crate::markdown;
use anyhow::Result;
use std::path::Path;
use tracing::{info, warn};

pub async fn maybe_run(
    cx: &Context,
    id: PaperId,
    version: &Canonical,
    report: &mut ReportBuilder,
) -> Result<()> {
    let root = &cx.cfg.cache_root;
    let paths = cache::version_paths(root, id, version);
    let vmeta = cache::read_version_meta(root, id, version).await;
    if paths.md.exists() && vmeta.md_converter.as_deref() == Some(markdown::CONVERTER_ID) {
        info!(id = %id, version = %version, "markdown already cached");
        return Ok(());
    }
    let converter_dir = markdown::CONVERTER_ID.replace('@', "-");
    remove_stale_page_caches(&paths.md_pages, &converter_dir);
    let md = markdown::convert(cx, &paths.pdf, &paths.md_pages.join(&converter_dir)).await?;
    tokio::fs::write(&paths.md, &md).await?;
    let mut vmeta = cache::read_version_meta(root, id, version).await;
    vmeta.md_converter = Some(markdown::CONVERTER_ID.to_owned());
    cache::write_version_meta(root, id, version, &vmeta).await?;
    report.action("converted-md");
    Ok(())
}

/// Page caches from other converters can never be reused; drop them.
fn remove_stale_page_caches(md_pages: &Path, keep: &str) {
    let Ok(entries) = std::fs::read_dir(md_pages) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name() != keep && entry.path().is_dir() {
            if let Err(e) = std::fs::remove_dir_all(entry.path()) {
                warn!(dir = %entry.path().display(), error = %e, "could not remove stale page cache");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_only_other_converters_page_caches() {
        let tmp = tempfile::tempdir().unwrap();
        for d in ["old-converter", "current"] {
            std::fs::create_dir(tmp.path().join(d)).unwrap();
        }
        remove_stale_page_caches(tmp.path(), "current");
        assert!(!tmp.path().join("old-converter").exists());
        assert!(tmp.path().join("current").exists());
        remove_stale_page_caches(&tmp.path().join("missing"), "current"); // no-op
    }
}
