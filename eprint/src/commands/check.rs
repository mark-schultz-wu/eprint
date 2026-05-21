//! `eprint check <ref>` — report cache staleness without touching the
//! network. (`--offline`-friendly; sync hook still runs unless --offline.)

use crate::cache;
use crate::cli::{CheckArgs, Context};
use crate::id::PaperRef;
use anyhow::{Context as _, Result};
use serde::Serialize;

#[derive(Debug, Serialize)]
struct CheckOutput {
    id: String,
    version: Option<u32>,
    in_cache: bool,
    is_stale: bool,
    latest_known_oai_datestamp: Option<String>,
    cached_oai_datestamp: Option<String>,
}

pub async fn run(cx: &Context, args: CheckArgs) -> Result<()> {
    let r: PaperRef = args.id.parse().context("parsing paper reference")?;
    crate::commands::sync::maybe_auto_sync(cx).await?;

    let root = &cx.cfg.cache_root;
    let paper_meta = cache::read_paper_meta(root, r.id).await;
    let in_cache = paper_meta.is_some();
    let version = r.version.or_else(|| paper_meta.as_ref().and_then(|pm| pm.current_version));

    let vmeta = match version {
        Some(v) if in_cache => Some(cache::read_version_meta(root, r.id, v).await),
        _ => None,
    };
    let cached_ds = vmeta.as_ref().and_then(|v| v.oai_datestamp.clone());
    let latest_known = paper_meta
        .as_ref()
        .and_then(|pm| pm.latest_known_oai_datestamp.clone());
    let stale = match (latest_known.as_deref(), cached_ds.as_deref()) {
        (Some(seen), Some(have)) => crate::oai::datestamp_cmp(seen, have)
            .context("comparing cached vs latest OAI datestamps")?
            .is_gt(),
        (Some(_), None) if in_cache => true,
        _ => false,
    };

    let out = CheckOutput {
        id: r.id.canonical(),
        version,
        in_cache,
        is_stale: stale,
        latest_known_oai_datestamp: latest_known,
        cached_oai_datestamp: cached_ds,
    };

    if cx.json {
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        if !in_cache {
            println!("{} — not in cache", out.id);
        } else if stale {
            println!(
                "{} v{} — STALE (cached datestamp: {}, latest seen: {})",
                out.id,
                version.unwrap(),
                out.cached_oai_datestamp.as_deref().unwrap_or("(none)"),
                out.latest_known_oai_datestamp.as_deref().unwrap_or("(none)"),
            );
        } else {
            println!("{} v{} — fresh", out.id, version.unwrap());
        }
    }
    Ok(())
}
