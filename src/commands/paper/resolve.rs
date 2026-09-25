//! Pick which version of a paper the caller wants. Priority:
//! 1. `--at <version>` (explicit pin)
//! 2. `--select-version` (interactive picker via `dialoguer`)
//! 3. `paper_meta.current_version` (the default)

use crate::cache::{self, PaperMeta};
use crate::cli::{Context, PaperArgs};
use crate::ids::version::Canonical;
use crate::ids::PaperId;
use anyhow::{Context as _, Result};

pub async fn target_version(
    cx: &Context,
    id: PaperId,
    paper_meta: Option<&PaperMeta>,
    args: &PaperArgs,
) -> Result<Option<Canonical>> {
    if let Some(v) = &args.at {
        let parsed: Canonical = v
            .parse()
            .with_context(|| format!("--at {v:?} is not a version timestamp (YYYYMMDDTHHMMSSZ)"))?;
        return Ok(Some(parsed));
    }
    if args.select_version {
        anyhow::ensure!(
            !cx.json,
            "--select-version is interactive; incompatible with --json"
        );
        let versions: Vec<Canonical> = paper_meta
            .map(|p| p.known_versions.clone())
            .unwrap_or_default();
        anyhow::ensure!(
            !versions.is_empty(),
            "no known versions to choose from (run without --offline to scrape the archive)"
        );
        let labels: Vec<String> = versions
            .iter()
            .map(|v| label_for(cx, id, paper_meta, v))
            .collect();
        let idx = dialoguer::Select::new()
            .with_prompt("Pick a version")
            .items(&labels)
            .default(labels.len() - 1)
            .interact()?;
        return Ok(Some(versions[idx]));
    }
    Ok(paper_meta.and_then(|p| p.current_version))
}

fn label_for(cx: &Context, id: PaperId, paper_meta: Option<&PaperMeta>, v: &Canonical) -> String {
    let is_current = paper_meta
        .and_then(|p| p.current_version.as_ref())
        .map(|c| c == v)
        .unwrap_or(false);
    let is_cached = cache::version_dir(&cx.cfg.cache_root, id, v).exists();
    let mut tags = Vec::new();
    if is_current {
        tags.push("current");
    }
    if is_cached {
        tags.push("cached");
    }
    if tags.is_empty() {
        v.to_string()
    } else {
        format!("{v}   ({})", tags.join(", "))
    }
}
