//! Produce `paper.md` for one cached version (see [`crate::markdown`]).
//!
//! Cached Markdown is reused only if the current converter produced it;
//! anything older (e.g. from the retired MinerU 3 / pdf-extract tiers) is
//! regenerated.

use crate::cache;
use crate::cli::Context;
use crate::commands::paper::ReportBuilder;
use crate::id::PaperId;
use crate::markdown;
use crate::version::Canonical;
use anyhow::Result;
use tracing::info;

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
    let pages_dir = paths.dir.join("md-pages").join(markdown::CONVERTER_ID.replace('@', "-"));
    let md = markdown::convert(cx, &paths.pdf, &pages_dir).await?;
    tokio::fs::write(&paths.md, &md).await?;
    let mut vmeta = cache::read_version_meta(root, id, version).await;
    vmeta.md_converter = Some(markdown::CONVERTER_ID.to_owned());
    cache::write_version_meta(root, id, version, &vmeta).await?;
    report.action("converted-md");
    Ok(())
}
