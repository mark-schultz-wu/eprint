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
    current_version: Option<crate::version::Canonical>,
    versions: Vec<crate::version::Canonical>,
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
            // Skip directories without one of our paper-meta files —
            // matches `clear`'s positive-identification policy.
            let Some(pm) = cache::read_paper_meta(root, id).await else {
                continue;
            };
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
            let v_str = match (p.current_version.as_ref(), p.versions.len()) {
                (Some(cv), 1) => cv.to_string(),
                (Some(cv), n) => format!("{cv} ({n} cached versions)"),
                (None, _) => "?".into(),
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

    // Identify which year/<num> subtrees actually belong to us, by
    // checking each paper's meta.json has the right tool tag. Anything
    // unrecognized is left alone — protects against `EPRINT_CACHE_DIR=$HOME`
    // accidentally nuking unrelated year-numbered dirs.
    let mut to_remove: Vec<std::path::PathBuf> = Vec::new();
    let mut bytes = 0u64;
    let mut foreign = 0u64;
    if let Ok(rd) = std::fs::read_dir(root) {
        for year in rd.flatten() {
            if !year.file_name().to_string_lossy().chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let Ok(num_rd) = std::fs::read_dir(year.path()) else { continue };
            for paper in num_rd.flatten() {
                let paper_path = paper.path();
                if !paper_path.is_dir() {
                    continue;
                }
                let meta_path = paper_path.join(cache::files::PAPER_META);
                if is_eprint_paper(&meta_path) {
                    bytes += dir_size(&paper_path);
                    to_remove.push(paper_path);
                } else {
                    foreign += 1;
                }
            }
        }
    }
    let papers = to_remove.len() as u64;
    if dry_run {
        println!(
            "would delete {} papers, {} from {}",
            papers,
            fmt_bytes(bytes),
            root.display()
        );
        if foreign > 0 {
            println!(
                "  ({} directories did NOT have an eprint meta.json and would be left in place)",
                foreign
            );
        }
        return Ok(());
    }
    for p in &to_remove {
        std::fs::remove_dir_all(p)?;
        // Try to remove the year-dir if it's now empty; ignore failure.
        if let Some(parent) = p.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }
    println!(
        "deleted {} papers, {} from {}",
        papers,
        fmt_bytes(bytes),
        root.display()
    );
    if foreign > 0 {
        println!(
            "  ({} unrecognized directories left in place)",
            foreign
        );
    }
    Ok(())
}

/// True iff `meta_path` exists and its JSON has `"tool": "eprint"`.
fn is_eprint_paper(meta_path: &Path) -> bool {
    let Ok(s) = std::fs::read_to_string(meta_path) else { return false };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) else { return false };
    v.get("tool").and_then(|t| t.as_str()) == Some(cache::TOOL_TAG)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn is_eprint_paper_accepts_correct_tag() {
        let tmp = tempfile_dir();
        let meta = tmp.path().join("meta.json");
        fs::write(&meta, r#"{"tool":"eprint","current_version":1}"#).unwrap();
        assert!(is_eprint_paper(&meta));
    }

    #[test]
    fn is_eprint_paper_rejects_missing_tag() {
        let tmp = tempfile_dir();
        let meta = tmp.path().join("meta.json");
        fs::write(&meta, r#"{"current_version":1}"#).unwrap();
        assert!(!is_eprint_paper(&meta));
    }

    #[test]
    fn is_eprint_paper_rejects_wrong_tag() {
        let tmp = tempfile_dir();
        let meta = tmp.path().join("meta.json");
        fs::write(&meta, r#"{"tool":"someone-else"}"#).unwrap();
        assert!(!is_eprint_paper(&meta));
    }

    #[test]
    fn is_eprint_paper_rejects_missing_file() {
        let tmp = tempfile_dir();
        let meta = tmp.path().join("meta.json"); // never written
        assert!(!is_eprint_paper(&meta));
    }

    #[test]
    fn is_eprint_paper_rejects_invalid_json() {
        let tmp = tempfile_dir();
        let meta = tmp.path().join("meta.json");
        fs::write(&meta, "not json").unwrap();
        assert!(!is_eprint_paper(&meta));
    }

    fn tempfile_dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("create tempdir")
    }
}
