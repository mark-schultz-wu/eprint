//! Optionally invoke `papermd` to produce Markdown for one cached version.
//! Implements the same cache ratchet as the previous standalone `convert`
//! verb (ml > text; never downgrades cached output).

use crate::cache;
use crate::cli::Context;
use crate::id::PaperId;
use crate::commands::paper::PaperReport;
use anyhow::{Context as _, Result};
use papermd::{Converter, LocalConverter, Quality, RemoteConverter};
use tracing::info;

pub async fn maybe_run(
    cx: &Context,
    id: PaperId,
    version: &str,
    quality: Quality,
    report: &mut PaperReport,
) -> Result<()> {
    let root = &cx.cfg.cache_root;
    let paths = cache::version_paths(root, id, version);
    let vmeta = cache::read_version_meta(root, id, version).await;
    let cached_q = parse_quality(vmeta.md_quality.as_deref());
    if paths.md.exists() && quality_at_least(cached_q, quality) {
        info!(id = %id, version, quality = ?cached_q, "markdown already cached at requested quality");
        return Ok(());
    }
    let markdown = match quality {
        Quality::Text => {
            let pdf_path = paths.pdf.clone();
            tokio::task::spawn_blocking(move || pdf_extract::extract_text(&pdf_path))
                .await
                .context("pdf-extract task panicked")?
                .context("pdf-extract failed")?
        }
        Quality::Ml => run_ml_backend(cx, &paths.pdf).await?,
    };
    tokio::fs::write(&paths.md, &markdown).await?;
    let mut vmeta = cache::read_version_meta(root, id, version).await;
    vmeta.md_quality = Some(match quality { Quality::Text => "text".into(), Quality::Ml => "ml".into() });
    if quality == Quality::Ml {
        vmeta.mineru_version = Some(papermd::local::MINERU_VERSION.to_owned());
    }
    cache::write_version_meta(root, id, version, &vmeta).await?;
    report.actions.push(if quality == Quality::Text { "converted-text" } else { "converted-ml" });
    Ok(())
}

async fn run_ml_backend(cx: &Context, pdf_path: &std::path::Path) -> Result<String> {
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
    let result = conv.convert(pdf_path, Quality::Ml).await?;
    Ok(result.markdown)
}

fn parse_quality(s: Option<&str>) -> Option<Quality> {
    match s? {
        "text" => Some(Quality::Text),
        "ml" => Some(Quality::Ml),
        _ => None,
    }
}

fn quality_at_least(cached: Option<Quality>, requested: Quality) -> bool {
    match (cached, requested) {
        (None, _) => false,
        (Some(Quality::Ml), _) => true,
        (Some(Quality::Text), Quality::Text) => true,
        (Some(Quality::Text), Quality::Ml) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ratchet_rules() {
        assert!(!quality_at_least(None, Quality::Text));
        assert!(quality_at_least(Some(Quality::Ml), Quality::Ml));
        assert!(quality_at_least(Some(Quality::Ml), Quality::Text));
        assert!(quality_at_least(Some(Quality::Text), Quality::Text));
        assert!(!quality_at_least(Some(Quality::Text), Quality::Ml));
    }
}
