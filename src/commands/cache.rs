//! `eprint cache {path,list,clear}`: report on and clear the local cache.

use crate::cache::{self, scan};
use crate::cli::{CacheArgs, CacheCommand, Context};
use crate::commands::format::{count, fmt_bytes};
use crate::ids::version::Canonical;
use crate::markdown::weights;
use anyhow::Result;
use serde::Serialize;

pub async fn run(cx: &Context, args: CacheArgs) -> Result<()> {
    match args.command {
        CacheCommand::Path => {
            println!("{}", cx.cfg.cache_root.display());
            Ok(())
        }
        CacheCommand::List => list(cx),
        CacheCommand::Clear { dry_run, models } => clear(cx, dry_run, models),
    }
}

#[derive(Debug, Serialize)]
struct ListedPaper {
    id: String,
    current_version: Option<Canonical>,
    versions: Vec<Canonical>,
    total_bytes: u64,
}

fn list(cx: &Context) -> Result<()> {
    let root = &cx.cfg.cache_root;
    let papers: Vec<ListedPaper> = scan::scan(root)
        .papers
        .into_iter()
        .map(|p| ListedPaper {
            id: p.id.canonical(),
            current_version: p.meta.and_then(|m| m.current_version),
            total_bytes: scan::dir_size(&p.dir),
            versions: p.versions,
        })
        .collect();

    if cx.json {
        println!("{}", serde_json::to_string_pretty(&papers)?);
        return Ok(());
    }
    if papers.is_empty() {
        println!("(no cached papers in {})", root.display());
    } else {
        println!("{} in {}", count(papers.len(), "paper"), root.display());
        for p in &papers {
            let versions = match (&p.current_version, p.versions.len()) {
                (Some(cv), 1) => cv.to_string(),
                (Some(cv), n) => format!("{cv} ({n} cached versions)"),
                (None, _) => "?".into(),
            };
            println!("  {}  {:>10}  {versions}", p.id, fmt_bytes(p.total_bytes));
        }
        let total: u64 = papers.iter().map(|p| p.total_bytes).sum();
        println!("  total: {}", fmt_bytes(total));
    }
    let models_dir = cache::models_dir(root);
    match scan::dir_size(&models_dir) {
        0 => println!(
            "Markdown model: not downloaded ({}, fetched on first --md)",
            fmt_bytes(weights::download_bytes())
        ),
        bytes => println!(
            "Markdown model: {} in {}",
            fmt_bytes(bytes),
            models_dir.display()
        ),
    }
    Ok(())
}

fn clear(cx: &Context, dry_run: bool, models: bool) -> Result<()> {
    let root = &cx.cfg.cache_root;
    let found = scan::scan(root);
    let bytes: u64 = found.papers.iter().map(|p| scan::dir_size(&p.dir)).sum();
    let models_dir = cache::models_dir(root);
    let model_bytes = scan::dir_size(&models_dir);

    if !dry_run {
        for paper in &found.papers {
            scan::remove_paper(paper)?;
        }
        if models && model_bytes > 0 {
            std::fs::remove_dir_all(&models_dir)?;
        }
    }

    let (verb, delete, keep) = if dry_run {
        ("would delete", "would delete", "would keep")
    } else {
        ("deleted", "deleted", "kept")
    };
    println!(
        "{verb} {}, {} from {}",
        count(found.papers.len(), "paper"),
        fmt_bytes(bytes),
        root.display()
    );
    if found.foreign > 0 {
        println!(
            "  ({} without an eprint meta.json {} left in place)",
            count(found.foreign, "numbered directory"),
            match (dry_run, found.foreign) {
                (true, _) => "would be",
                (false, 1) => "was",
                (false, _) => "were",
            },
        );
    }
    // The model is kept unless asked for: it's expensive to re-download.
    if model_bytes > 0 {
        if models {
            println!("{delete} the Markdown model, {}", fmt_bytes(model_bytes));
        } else {
            println!(
                "{keep} the Markdown model ({}); add --models to delete it too",
                fmt_bytes(model_bytes)
            );
        }
    }
    Ok(())
}
