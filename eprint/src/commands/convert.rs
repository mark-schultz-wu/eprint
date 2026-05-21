//! `eprint convert <id> [--quality=text|ml]` — produce a Markdown file
//! from a paper's cached PDF. Auto-fetches the PDF if missing.

use crate::cache::{self, Meta};
use crate::cli::{ConvertArgs, Context};
use crate::commands::fetch;
use crate::id::PaperId;
use anyhow::{Context as _, Result};
use papermd::{Converter, LocalConverter, Quality, RemoteConverter};
use std::path::Path;
use tracing::info;

pub async fn run(cx: &Context, args: ConvertArgs) -> Result<()> {
    let id: PaperId = args.id.parse().context("parsing paper id")?;
    let requested: Quality = args.quality.into();
    let paths = cache::paths_for(&cx.cfg.cache_root, id);

    // Auto-fetch PDF if missing. Skips network if already cached.
    if !paths.pdf.exists() {
        info!(id = %id, "PDF not cached; auto-fetching");
        fetch::fetch_all(cx, id).await?;
    }

    // Cache ratchet: if we already have a >= requested quality, reuse it.
    let meta = read_meta(&paths.meta).await;
    let cached_q = meta.md_quality.as_deref().and_then(parse_quality);
    if paths.md.exists() && quality_at_least(cached_q, requested) {
        info!(id = %id, quality = ?cached_q, "using cached markdown");
        let md = tokio::fs::read_to_string(&paths.md).await?;
        return emit(cx, &args, &md, cached_q.unwrap_or(requested));
    }

    // Convert.
    let conv = match requested {
        Quality::Text => convert_text(&paths.pdf).await?,
        Quality::Ml => convert_ml(cx, &paths.pdf).await?,
    };

    tokio::fs::write(&paths.md, &conv.markdown).await?;
    let mut meta = meta;
    meta.md_quality = Some(match conv.quality {
        Quality::Text => "text".into(),
        Quality::Ml => "ml".into(),
    });
    if conv.quality == Quality::Ml {
        meta.mineru_version = Some(papermd::local::MINERU_VERSION.to_owned());
    }
    tokio::fs::write(&paths.meta, serde_json::to_vec_pretty(&meta)?).await?;
    info!(id = %id, quality = ?conv.quality, dur_ms = conv.duration.as_millis() as u64, "convert complete");
    emit(cx, &args, &conv.markdown, conv.quality)
}

/// Text-only Rust-native conversion via the `pdf-extract` crate.
async fn convert_text(pdf_path: &Path) -> Result<papermd::Conversion> {
    let pdf_path = pdf_path.to_path_buf();
    let start = std::time::Instant::now();
    // pdf-extract is sync + may take ~1s on a long paper; offload to a
    // blocking thread so the runtime isn't tied up.
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

/// ML-tier conversion, dispatching to the configured backend.
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
            eprintln!("wrote {} ({} bytes, quality={})", path.display(), md.len(), quality_name(q));
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

async fn read_meta(meta_path: &Path) -> Meta {
    match tokio::fs::read_to_string(meta_path).await {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => Meta::default(),
    }
}

fn parse_quality(s: &str) -> Option<Quality> {
    match s {
        "text" => Some(Quality::Text),
        "ml" => Some(Quality::Ml),
        _ => None,
    }
}

/// `ml` is strictly better than `text` for the ratchet.
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
