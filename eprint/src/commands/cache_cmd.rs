//! `eprint cache {path,list,clear}` — local cache management.

use crate::cache;
use crate::cli::{CacheArgs, CacheCommand, Context};
use anyhow::Result;
use serde::Serialize;
use std::path::Path;

pub async fn run(cx: &Context, args: CacheArgs) -> Result<()> {
    match args.command {
        CacheCommand::Path => {
            println!("{}", cx.cfg.cache_root.display());
            Ok(())
        }
        CacheCommand::List => list(cx).await,
        CacheCommand::Clear { dry_run } => clear(cx, dry_run).await,
    }
}

#[derive(Debug, Serialize)]
struct CachedPaper {
    id: String,
    current_version: Option<u32>,
    versions: Vec<u32>,
    total_bytes: u64,
}

async fn list(cx: &Context) -> Result<()> {
    let root = &cx.cfg.cache_root;
    let mut papers = Vec::new();
    if !root.exists() {
        if !cx.json {
            println!("(cache is empty: {})", root.display());
        } else {
            println!("[]");
        }
        return Ok(());
    }
    // Iterate <root>/<year>/<num>/.
    let mut years: Vec<_> = match std::fs::read_dir(root) {
        Ok(rd) => rd
            .flatten()
            .filter_map(|e| {
                let n = e.file_name();
                let s = n.to_str()?;
                let y: u16 = s.parse().ok()?;
                Some((y, e.path()))
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    years.sort_by_key(|(y, _)| *y);

    for (year, year_dir) in years {
        let mut nums: Vec<_> = match std::fs::read_dir(&year_dir) {
            Ok(rd) => rd
                .flatten()
                .filter_map(|e| {
                    let n = e.file_name();
                    let s = n.to_str()?;
                    let num: u32 = s.parse().ok()?;
                    Some((num, e.path()))
                })
                .collect(),
            Err(_) => continue,
        };
        nums.sort_by_key(|(n, _)| *n);
        for (num, paper_dir) in nums {
            let id = crate::id::PaperId { year, num };
            let pm = cache::read_paper_meta(root, id).await;
            let versions = cache::existing_versions(root, id);
            let total_bytes = dir_size(&paper_dir);
            papers.push(CachedPaper {
                id: id.canonical(),
                current_version: pm.current_version,
                versions,
                total_bytes,
            });
        }
    }

    if cx.json {
        println!("{}", serde_json::to_string_pretty(&papers)?);
    } else if papers.is_empty() {
        println!("(cache is empty: {})", root.display());
    } else {
        println!("{} papers in {}", papers.len(), root.display());
        let mut total = 0u64;
        for p in &papers {
            let v_str = if p.versions.len() <= 1 {
                p.current_version.map(|v| format!("v{v}")).unwrap_or_else(|| "?".into())
            } else {
                format!(
                    "v1..v{} ({} versions, current v{})",
                    p.versions.last().unwrap(),
                    p.versions.len(),
                    p.current_version.unwrap_or(0),
                )
            };
            println!("  {}  {:>10}  {}", p.id, fmt_bytes(p.total_bytes), v_str);
            total += p.total_bytes;
        }
        println!("  total: {}", fmt_bytes(total));
    }
    Ok(())
}

async fn clear(cx: &Context, dry_run: bool) -> Result<()> {
    let root = &cx.cfg.cache_root;
    if !root.exists() {
        println!("(cache is empty: {})", root.display());
        return Ok(());
    }
    // Count what we'd remove.
    let mut papers = 0u64;
    let mut bytes = 0u64;
    if let Ok(rd) = std::fs::read_dir(root) {
        for year in rd.flatten() {
            if !year.file_name().to_string_lossy().chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            if let Ok(num_rd) = std::fs::read_dir(year.path()) {
                for paper in num_rd.flatten() {
                    if paper.path().is_dir() {
                        papers += 1;
                        bytes += dir_size(&paper.path());
                    }
                }
            }
        }
    }
    if dry_run {
        println!(
            "would delete {} papers, {} from {}",
            papers,
            fmt_bytes(bytes),
            root.display()
        );
        return Ok(());
    }
    // Actually delete: remove only the year/<num>/ subtrees, leave
    // .rate_limit_stamp / .last_sync_unix_s alone.
    if let Ok(rd) = std::fs::read_dir(root) {
        for year in rd.flatten() {
            if year.file_name().to_string_lossy().chars().all(|c| c.is_ascii_digit()) {
                std::fs::remove_dir_all(year.path())?;
            }
        }
    }
    println!(
        "deleted {} papers, {} from {}",
        papers,
        fmt_bytes(bytes),
        root.display()
    );
    Ok(())
}

fn dir_size(p: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![p.to_path_buf()];
    while let Some(d) = stack.pop() {
        if let Ok(rd) = std::fs::read_dir(&d) {
            for entry in rd.flatten() {
                let path = entry.path();
                match entry.metadata() {
                    Ok(m) if m.is_file() => total += m.len(),
                    Ok(m) if m.is_dir() => stack.push(path),
                    _ => {}
                }
            }
        }
    }
    total
}

fn fmt_bytes(n: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
    let mut f = n as f64;
    let mut u = 0;
    while f >= 1024.0 && u < UNITS.len() - 1 {
        f /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{f:.1} {}", UNITS[u])
    }
}
