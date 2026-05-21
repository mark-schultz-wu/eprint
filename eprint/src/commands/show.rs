//! `eprint show <ref>` — print cached metadata for a paper.
//!
//! Default: human-readable. With `--json`: a structured envelope including
//! paper meta + the active version's meta + the cached abstract text.

use crate::cache;
use crate::cli::{Context, ShowArgs};
use crate::id::PaperRef;
use anyhow::{Context as _, Result};
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Serialize)]
struct ShowOutput {
    id: String,
    version: u32,
    directory: String,
    title: Option<String>,
    abstract_: Option<String>,
    bibtex: Option<String>,
    current_version: Option<u32>,
    latest_known_oai_datestamp: Option<String>,
    version_meta: VersionView,
    is_stale: bool,
}

#[derive(Debug, Serialize)]
struct VersionView {
    fetched_unix_s: Option<i64>,
    oai_datestamp: Option<String>,
    md_quality: Option<String>,
    mineru_version: Option<String>,
}

pub async fn run(cx: &Context, args: ShowArgs) -> Result<()> {
    let r: PaperRef = args.id.parse().context("parsing paper reference")?;
    crate::commands::sync::maybe_auto_sync(cx).await?;

    let root = &cx.cfg.cache_root;
    let paper_meta = cache::read_paper_meta(root, r.id).await;
    let version = r.version.or(paper_meta.current_version).ok_or_else(|| {
        anyhow::anyhow!(
            "{} is not in the cache; run `eprint fetch {}` first",
            r.id,
            r.id
        )
    })?;
    let paths = cache::version_paths(root, r.id, version);
    anyhow::ensure!(
        paths.dir.exists(),
        "{} v{version} not cached",
        r.id
    );
    let vmeta = cache::read_version_meta(root, r.id, version).await;

    let abstract_ = read_optional(&paths.abstract_).await;
    let bibtex = read_optional(&paths.bib).await;

    let stale = match (
        paper_meta.latest_known_oai_datestamp.as_deref(),
        vmeta.oai_datestamp.as_deref(),
    ) {
        (Some(seen), Some(have)) => seen > have,
        (Some(_), None) => true,
        _ => false,
    };

    let out = ShowOutput {
        id: r.id.canonical(),
        version,
        directory: paths.dir.display().to_string(),
        title: paper_meta.title.clone(),
        abstract_,
        bibtex,
        current_version: paper_meta.current_version,
        latest_known_oai_datestamp: paper_meta.latest_known_oai_datestamp.clone(),
        version_meta: VersionView {
            fetched_unix_s: vmeta.fetched_unix_s,
            oai_datestamp: vmeta.oai_datestamp.clone(),
            md_quality: vmeta.md_quality.clone(),
            mineru_version: vmeta.mineru_version.clone(),
        },
        is_stale: stale,
    };

    if cx.json {
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        human_summary(&out);
    }
    Ok(())
}

async fn read_optional(p: &PathBuf) -> Option<String> {
    tokio::fs::read_to_string(p).await.ok()
}

fn human_summary(o: &ShowOutput) {
    println!("{}", o.id);
    if let Some(t) = &o.title {
        println!("  title:                {t}");
    }
    println!("  version:              v{}", o.version);
    if let Some(c) = o.current_version {
        if c != o.version {
            println!("  (current version is v{c})");
        }
    }
    if let Some(q) = &o.version_meta.md_quality {
        println!("  markdown quality:     {q}");
    }
    if let Some(v) = &o.version_meta.mineru_version {
        println!("  mineru version:       {v}");
    }
    if let Some(ds) = &o.version_meta.oai_datestamp {
        println!("  oai datestamp:        {ds}");
    }
    if let Some(ds) = &o.latest_known_oai_datestamp {
        println!("  latest known (sync):  {ds}");
    }
    if o.is_stale {
        println!("  status:               STALE — newer OAI datestamp seen; next `fetch` will pull a new version");
    }
    println!("  dir:                  {}", o.directory);
    if let Some(abs) = &o.abstract_ {
        println!();
        println!("Abstract:");
        for line in abs.lines() {
            println!("  {line}");
        }
    }
}
