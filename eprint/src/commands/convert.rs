//! `eprint convert <ref>` — produce Markdown from a paper's cached PDF.
//!
//! Operates on a specific version (`vN`). Default version is the paper's
//! `current_version` (from `meta.json`); override with `2024/463@v2`.
//!
//! The conversion is cached **per-version** at `vN/paper.md`. A cache
//! ratchet prevents downgrading: if `vN/paper.md` already exists at `ml`
//! quality, a request for `text` quality is served from the cached `ml`
//! output rather than regenerating.

use crate::cache;
use crate::cli::{ConvertArgs, Context};
use crate::commands::fetch;
use crate::id::{PaperRef};
use anyhow::{Context as _, Result};
use papermd::{Converter, LocalConverter, Quality, RemoteConverter};
use std::path::Path;
use tracing::info;

pub async fn run(cx: &Context, args: ConvertArgs) -> Result<()> {
    let r: PaperRef = args.id.parse().context("parsing paper reference")?;
    let requested: Quality = args.quality.into();
    crate::commands::sync::maybe_auto_sync(cx).await?;

    // Ensure we have a version on disk to convert. Auto-fetch via fetch_ref
    // (which handles version pins, staleness, and offline mode).
    let report = fetch::fetch_ref(cx, r).await?;
    let version = report.version;
    let paths = cache::version_paths(&cx.cfg.cache_root, r.id, version);

    let vmeta = cache::read_version_meta(&cx.cfg.cache_root, r.id, version).await;
    let cached_q = vmeta.md_quality.as_deref().and_then(parse_quality);

    if paths.md.exists() && quality_at_least(cached_q, requested) {
        info!(id = %r.id, version, quality = ?cached_q, "using cached markdown");
        let md = tokio::fs::read_to_string(&paths.md).await?;
        return emit(cx, &args, &md, cached_q.unwrap_or(requested));
    }

    let conv = match requested {
        Quality::Text => convert_text(&paths.pdf).await?,
        Quality::Ml => convert_ml(cx, &paths.pdf).await?,
    };

    tokio::fs::write(&paths.md, &conv.markdown).await?;
    let mut vmeta = vmeta;
    vmeta.md_quality = Some(match conv.quality {
        Quality::Text => "text".into(),
        Quality::Ml => "ml".into(),
    });
    if conv.quality == Quality::Ml {
        vmeta.mineru_version = Some(papermd::local::MINERU_VERSION.to_owned());
    }
    cache::write_version_meta(&cx.cfg.cache_root, r.id, version, &vmeta).await?;
    info!(
        id = %r.id,
        version,
        quality = ?conv.quality,
        dur_ms = conv.duration.as_millis() as u64,
        "convert complete"
    );
    emit(cx, &args, &conv.markdown, conv.quality)
}

/// Text-only Rust-native conversion via the `pdf-extract` crate.
async fn convert_text(pdf_path: &Path) -> Result<papermd::Conversion> {
    let pdf_path = pdf_path.to_path_buf();
    let start = std::time::Instant::now();
    let markdown = tokio::task::spawn_blocking(move || {
        pdf_extract::extract_text(&pdf_path)
            .with_context(|| format!("pdf-extract failed on {}", pdf_path.display()))
    })
    .await
    .context("pdf-extract task panicked")??;
    Ok(papermd::Conversion {
        markdown,
        quality: Quality::Text,
        duration: start.elapsed(),
    })
}

async fn convert_ml(cx: &Context, pdf_path: &Path) -> Result<papermd::Conversion> {
    use crate::config::BackendKind;
    let cfg = &cx.cfg.ml;
    match cfg.kind {
        BackendKind::Local => {
            let conv = LocalConverter::default();
            Ok(conv.convert(pdf_path, Quality::Ml).await?)
        }
        BackendKind::Remote => {
            let endpoint = cfg.endpoint.as_deref().ok_or_else(|| {
                anyhow::anyhow!("EPRINT_ML_BACKEND=remote requires EPRINT_ML_ENDPOINT")
            })?;
            let token = cfg
                .token_env
                .as_deref()
                .and_then(|var| std::env::var(var).ok());
            let mut conv = RemoteConverter::new(endpoint)?;
            if let Some(t) = token {
                conv = conv.with_token(t);
            }
            Ok(conv.convert(pdf_path, Quality::Ml).await?)
        }
    }
}

fn emit(cx: &Context, args: &ConvertArgs, md: &str, q: Quality) -> Result<()> {
    if let Some(path) = &args.output {
        std::fs::write(path, md)?;
        if cx.json {
            let r = serde_json::json!({
                "id": args.id,
                "output": path.display().to_string(),
                "quality": q,
                "bytes": md.len(),
            });
            println!("{}", serde_json::to_string_pretty(&r)?);
        } else {
            eprintln!(
                "wrote {} ({} bytes, quality={})",
                path.display(),
                md.len(),
                quality_name(q)
            );
        }
    } else if cx.json {
        let r = serde_json::json!({
            "id": args.id,
            "quality": q,
            "markdown": md,
        });
        println!("{}", serde_json::to_string_pretty(&r)?);
    } else {
        print!("{md}");
    }
    Ok(())
}

fn parse_quality(s: &str) -> Option<Quality> {
    match s {
        "text" => Some(Quality::Text),
        "ml" => Some(Quality::Ml),
        _ => None,
    }
}

/// `ml` strictly dominates `text` for the ratchet.
fn quality_at_least(cached: Option<Quality>, requested: Quality) -> bool {
    match (cached, requested) {
        (None, _) => false,
        (Some(Quality::Ml), _) => true,
        (Some(Quality::Text), Quality::Text) => true,
        (Some(Quality::Text), Quality::Ml) => false,
    }
}

fn quality_name(q: Quality) -> &'static str {
    match q {
        Quality::Text => "text",
        Quality::Ml => "ml",
    }
}
