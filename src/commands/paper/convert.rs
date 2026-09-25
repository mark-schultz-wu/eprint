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
    let converted = markdown::convert(cx, &paths.pdf, &paths.md_pages.join(&converter_dir)).await?;
    record(root, id, version, &converted, report).await
}

/// Write the conversion's Markdown, and mark the version converted only if
/// every page succeeded; otherwise report a partial conversion.
async fn record(
    root: &Path,
    id: PaperId,
    version: &Canonical,
    converted: &markdown::Converted,
    report: &mut ReportBuilder,
) -> Result<()> {
    let paths = cache::version_paths(root, id, version);
    tokio::fs::create_dir_all(&paths.dir).await?;
    tokio::fs::write(&paths.md, &converted.markdown).await?;
    if !converted.failed_pages.is_empty() {
        // Leave md_converter unset: the next `--md` retries the failed pages
        // (the others are cached) instead of treating this as done.
        let pages: Vec<String> = converted
            .failed_pages
            .iter()
            .map(|p| p.to_string())
            .collect();
        return Err(crate::exit::CommandFailure::PartialConversion(format!(
            "converted {id} version {version} except page(s) {} (placeholders mark them in \
             {}); re-run with --md to retry just those pages",
            pages.join(", "),
            paths.md.display(),
        ))
        .into());
    }
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

    fn v() -> Canonical {
        "20240319T143540Z".parse().unwrap()
    }

    const ID: PaperId = PaperId {
        year: 2024,
        num: 463,
    };

    #[tokio::test]
    async fn a_complete_conversion_is_marked_done() {
        let root = tempfile::tempdir().unwrap();
        let mut report = ReportBuilder::new(ID.canonical());
        let converted = markdown::Converted {
            markdown: "# Paper\n".into(),
            failed_pages: vec![],
        };
        record(root.path(), ID, &v(), &converted, &mut report)
            .await
            .unwrap();
        let paths = cache::version_paths(root.path(), ID, &v());
        assert_eq!(std::fs::read_to_string(paths.md).unwrap(), "# Paper\n");
        let meta = cache::read_version_meta(root.path(), ID, &v()).await;
        assert_eq!(meta.md_converter.as_deref(), Some(markdown::CONVERTER_ID));
    }

    /// A partial conversion keeps its Markdown but isn't marked done, so the
    /// next `--md` retries the failed pages; it exits with code 5.
    #[tokio::test]
    async fn a_partial_conversion_is_written_but_not_marked_done() {
        let root = tempfile::tempdir().unwrap();
        let mut report = ReportBuilder::new(ID.canonical());
        let converted = markdown::Converted {
            markdown: "page 1\n\n<!-- page 2 could not be converted: boom -->\n".into(),
            failed_pages: vec![2, 5],
        };
        let err = record(root.path(), ID, &v(), &converted, &mut report)
            .await
            .unwrap_err();
        assert_eq!(crate::exit::CommandFailure::code_of(&err), 5);
        assert!(err.to_string().contains("except page(s) 2, 5"), "{err}");
        let paths = cache::version_paths(root.path(), ID, &v());
        assert!(std::fs::read_to_string(paths.md)
            .unwrap()
            .contains("could not be converted"));
        let meta = cache::read_version_meta(root.path(), ID, &v()).await;
        assert_eq!(meta.md_converter, None);
    }

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
