//! Finding the papers in a cache, and sizing and removing them.
//!
//! A directory `<root>/<year>/<num>/` is one of ours iff `year` and `num`
//! are numeric and its `meta.json` carries `"tool": "eprint"` ([`TOOL_TAG`]).
//! The tag check is what makes `eprint cache clear` safe even if
//! `EPRINT_CACHE_DIR` points somewhere shared (e.g. `$HOME`): numbered
//! directories we didn't write count as foreign and are left alone.
//! Non-numeric entries in the root (`models/`, the sync stamp) are not papers.

use super::{files, versions_in, PaperMeta, TOOL_TAG};
use crate::ids::version::Canonical;
use crate::ids::PaperId;
use std::io;
use std::path::{Path, PathBuf};

/// A paper directory positively identified as ours.
#[derive(Debug)]
pub struct CachedPaper {
    pub id: PaperId,
    pub dir: PathBuf,
    /// `None` if the directory is tagged as ours but its meta doesn't parse
    /// (e.g. written by a newer version of the tool).
    pub meta: Option<PaperMeta>,
    /// Version subdirectories present, ascending.
    pub versions: Vec<Canonical>,
}

#[derive(Debug, Default)]
pub struct Scan {
    /// Ours, sorted by year then number.
    pub papers: Vec<CachedPaper>,
    /// Numbered directories that aren't ours.
    pub foreign: usize,
}

/// Enumerate the cache.
pub fn scan(root: &Path) -> Scan {
    let mut out = Scan::default();
    for (year, year_dir) in numbered_subdirs(root) {
        for (num, dir) in numbered_subdirs(&year_dir) {
            match identify(dir, year, num) {
                Some(paper) => out.papers.push(paper),
                None => out.foreign += 1,
            }
        }
    }
    out
}

/// Whether the cache holds at least one paper (stops at the first).
pub fn has_any_paper(root: &Path) -> bool {
    numbered_subdirs(root).into_iter().any(|(year, year_dir)| {
        numbered_subdirs(&year_dir)
            .into_iter()
            .any(|(num, dir)| identify(dir, year, num).is_some())
    })
}

/// Delete a paper's directory, and its year directory if that leaves it empty.
pub fn remove_paper(paper: &CachedPaper) -> io::Result<()> {
    std::fs::remove_dir_all(&paper.dir)?;
    if let Some(year_dir) = paper.dir.parent() {
        // Fails (harmlessly) unless the year directory is now empty.
        let _ = std::fs::remove_dir(year_dir);
    }
    Ok(())
}

/// Total size of the regular files under `path` (0 if it doesn't exist).
pub fn dir_size(path: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            match entry.metadata() {
                Ok(m) if m.is_file() => total += m.len(),
                Ok(m) if m.is_dir() => stack.push(entry.path()),
                _ => {}
            }
        }
    }
    total
}

/// Subdirectories of `dir` whose names are all digits, with their numeric
/// value (`None` if too large to be a year/number), sorted by value.
fn numbered_subdirs(dir: &Path) -> Vec<(Option<u32>, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(Option<u32>, PathBuf)> = rd
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let all_digits = !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit());
            all_digits.then(|| (name.parse().ok(), e.path()))
        })
        .collect();
    out.sort();
    out
}

/// `Some` iff `dir` is one of ours (see the module docs).
fn identify(dir: PathBuf, year: Option<u32>, num: Option<u32>) -> Option<CachedPaper> {
    let raw = std::fs::read_to_string(dir.join(files::META)).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    if value.get("tool")?.as_str()? != TOOL_TAG {
        return None;
    }
    let id = PaperId {
        year: u16::try_from(year?).ok()?,
        num: num?,
    };
    Some(CachedPaper {
        id,
        meta: serde_json::from_value(value).ok(),
        versions: versions_in(&dir),
        dir,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const OURS: &str = r#"{"tool":"eprint","current_version":"20240319T143540Z"}"#;

    fn paper(root: &Path, rel: &str, meta: Option<&str>) -> PathBuf {
        let dir = root.join(rel);
        fs::create_dir_all(&dir).unwrap();
        if let Some(m) = meta {
            fs::write(dir.join("meta.json"), m).unwrap();
        }
        dir
    }

    #[test]
    fn identifies_ours_and_counts_foreign() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let ours = paper(root, "2024/0463", Some(OURS));
        fs::create_dir(ours.join("20240319T143540Z")).unwrap();
        paper(
            root,
            "2023/0001",
            Some(r#"{"tool":"eprint","current_version":1}"#),
        );
        paper(root, "2024/0007", Some(r#"{"tool":"someone-else"}"#));
        paper(root, "2024/0008", None);
        paper(root, "2024/0009", Some("not json"));
        paper(root, "models/MinerU", Some(OURS)); // not numbered: ignored
        fs::write(root.join("2024/notes.txt"), "a file, not a paper").unwrap();

        let s = scan(root);
        let ids: Vec<String> = s.papers.iter().map(|p| p.id.canonical()).collect();
        assert_eq!(ids, ["2023/001", "2024/463"]);
        assert!(
            s.papers[0].meta.is_none(),
            "tagged but unparseable is still ours"
        );
        let p = &s.papers[1];
        assert_eq!(p.dir, ours);
        assert_eq!(
            p.meta
                .as_ref()
                .unwrap()
                .current_version
                .unwrap()
                .to_string(),
            "20240319T143540Z"
        );
        assert_eq!(p.versions.len(), 1);
        assert_eq!(s.foreign, 3);
    }

    #[test]
    fn has_any_paper_needs_a_tagged_paper() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!has_any_paper(tmp.path()));
        paper(tmp.path(), "2024/0008", Some(r#"{"tool":"someone-else"}"#));
        assert!(!has_any_paper(tmp.path()));
        paper(tmp.path(), "2024/0463", Some(OURS));
        assert!(has_any_paper(tmp.path()));
        assert!(!has_any_paper(&tmp.path().join("missing")));
    }

    #[test]
    fn remove_paper_prunes_empty_year_dirs_only() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        paper(root, "2023/0001", Some(OURS));
        paper(root, "2024/0463", Some(OURS));
        paper(root, "2024/0007", None); // foreign neighbour keeps 2024/ alive
        for p in &scan(root).papers {
            remove_paper(p).unwrap();
        }
        assert!(!root.join("2023").exists());
        assert!(!root.join("2024/0463").exists());
        assert!(root.join("2024/0007").exists());
    }

    #[test]
    fn dir_size_sums_nested_files() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("a/b")).unwrap();
        fs::write(tmp.path().join("a/x"), [0u8; 10]).unwrap();
        fs::write(tmp.path().join("a/b/y"), [0u8; 5]).unwrap();
        assert_eq!(dir_size(tmp.path()), 15);
        assert_eq!(dir_size(&tmp.path().join("missing")), 0);
    }
}
