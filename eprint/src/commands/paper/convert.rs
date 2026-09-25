//! Optionally invoke `papermd` to produce Markdown for one cached version.
//! Implements the same cache ratchet as the previous standalone `convert`
//! verb (ml > text; never downgrades cached output).

use crate::cache;
use crate::cli::Context;
use crate::id::PaperId;
use crate::version::Canonical;
use crate::commands::paper::ReportBuilder;
use anyhow::{Context as _, Result};
use papermd::{Converter, LocalConverter, Quality, RemoteConverter};
use tracing::{debug, info, warn};

pub async fn maybe_run(
    cx: &Context,
    id: PaperId,
    version: &Canonical,
    quality: Quality,
    report: &mut ReportBuilder,
) -> Result<()> {
    let root = &cx.cfg.cache_root;
    let paths = cache::version_paths(root, id, version);
    let vmeta = cache::read_version_meta(root, id, version).await;
    let cached_q = parse_quality(vmeta.md_quality.as_deref());
    if paths.md.exists() && quality_at_least(cached_q, quality) {
        info!(id = %id, version = %version, quality = ?cached_q, "markdown already cached at requested quality");
        return Ok(());
    }
    let markdown = match quality {
        Quality::Text => {
            let pdf_path = paths.pdf.clone();
            debug!(id = %id, version = %version, pdf = %pdf_path.display(), "extracting text (pdf-extract)");
            let text = tokio::task::spawn_blocking({
                let pdf_path = pdf_path.clone();
                move || pdf_extract::extract_text(&pdf_path)
            })
            .await
            .context("pdf-extract task panicked")?
            .with_context(|| format!("pdf-extract failed on {}", pdf_path.display()))?;
            // pdf-extract can "succeed" with no text on scanned/image-only PDFs.
            // Writing that empty .md and ratcheting md_quality to "text" would
            // mask the real problem (no extractable text layer) behind exit 0.
            // Fail hard with a coded error so callers can detect it and retry
            // with ML/OCR — independent of whether the warn below is visible.
            if text.trim().is_empty() {
                warn!(
                    id = %id, version = %version, pdf = %pdf_path.display(),
                    "pdf-extract produced no text — likely a scanned/image-only PDF"
                );
                return Err(crate::exit::CommandFailure::EmptyConversion(format!(
                    "pdf-extract produced no text for {id} version {version} ({}); \
                     it's likely a scanned/image-only PDF with no text layer. \
                     Re-run with --md ml for OCR-quality conversion.",
                    pdf_path.display(),
                ))
                .into());
            }
            text
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
    report.action(if quality == Quality::Text { "converted-text" } else { "converted-ml" });
    Ok(())
}

async fn run_ml_backend(cx: &Context, pdf_path: &std::path::Path) -> Result<String> {
    use crate::config::BackendKind;
    let cfg = &cx.cfg.ml;
    let conv: Box<dyn Converter> = match cfg.kind {
        BackendKind::Local => {
            // The local backend shells out to MinerU on CPU, which runs for
            // minutes on math-heavy papers and prints nothing. Without this
            // notice it reads as a hang. (Suppressed under --json, like sync.)
            if !cx.json {
                eprintln!(
                    "Converting with MinerU {} locally (CPU) — this can take several minutes \
                     for math-heavy papers and shows no progress; it's working, not hung. \
                     Set EPRINT_ML_BACKEND=remote + EPRINT_ML_ENDPOINT to offload.",
                    papermd::local::MINERU_VERSION,
                );
            }
            Box::new(LocalConverter::default())
        }
        BackendKind::Remote => {
            let endpoint = cfg.endpoint.as_deref().ok_or_else(|| {
                anyhow::anyhow!("EPRINT_ML_BACKEND=remote requires EPRINT_ML_ENDPOINT")
            })?;
            if !cx.json {
                eprintln!("Converting via remote ML endpoint {endpoint} …");
            }
            let token = cfg.token_env.as_deref().and_then(|v| std::env::var(v).ok());
            let mut rc = RemoteConverter::new(endpoint)?;
            if let Some(t) = token { rc = rc.with_token(t); }
            Box::new(rc)
        }
    };
    let result = conv.convert(pdf_path, Quality::Ml).await?;
    if !cx.json {
        eprintln!("  ML conversion finished in {:.0}s.", result.duration.as_secs_f64());
    }
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
