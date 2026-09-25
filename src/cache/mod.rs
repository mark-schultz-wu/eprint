//! The on-disk cache: its layout, metadata, and enumeration.
//!
//! ```text
//! <cache_root>/
//!   .last_sync_unix_s              # when `eprint sync` last ran
//!   models/                        # Markdown model weights (see crate::markdown::weights)
//!   2024/
//!     0463/
//!       meta.json                  # PaperMeta
//!       20250106T174348Z/          # one dir per cached version
//!         meta.json                # VersionMeta
//!         paper.pdf
//!         paper.md
//!         paper.bib
//!         abstract.txt
//!         md-pages/<converter>/    # per-page conversion results, for resuming
//!       20240319T143540Z/
//!         ...
//! ```
//!
//! Version directory names use the canonical form from
//! [`crate::ids::version`] (filesystem-friendly basic ISO 8601 UTC). This
//! module is the only place that knows the layout: everything else asks it
//! for paths.
//!
//! - [`meta`]: paper- and version-level metadata files.
//! - [`scan`]: finding the papers in a cache, sizing and removing them.

mod meta;
pub mod scan;

pub use meta::{
    purge_if_outdated, read_last_sync, read_paper_meta, read_version_meta, write_last_sync,
    write_paper_meta, write_version_meta, PaperMeta, VersionMeta,
};

use crate::ids::version::Canonical;
use crate::ids::PaperId;
use std::path::{Path, PathBuf};

mod files {
    pub const PDF: &str = "paper.pdf";
    pub const MD: &str = "paper.md";
    pub const MD_PAGES: &str = "md-pages";
    pub const BIB: &str = "paper.bib";
    pub const ABSTRACT: &str = "abstract.txt";
    pub const META: &str = "meta.json";
    pub const MODELS: &str = "models";
    pub const LAST_SYNC: &str = ".last_sync_unix_s";
}

/// Magic field embedded in every paper-level `meta.json` so destructive
/// operations can positively identify our cache entries.
pub const TOOL_TAG: &str = "eprint";

pub fn paper_dir(root: &Path, id: PaperId) -> PathBuf {
    root.join(id.cache_subdir())
}

pub fn version_dir(root: &Path, id: PaperId, version: &Canonical) -> PathBuf {
    paper_dir(root, id).join(version.to_string())
}

fn paper_meta_path(root: &Path, id: PaperId) -> PathBuf {
    paper_dir(root, id).join(files::META)
}

/// Where downloaded model weights live.
pub fn models_dir(root: &Path) -> PathBuf {
    root.join(files::MODELS)
}

fn last_sync_path(root: &Path) -> PathBuf {
    root.join(files::LAST_SYNC)
}

pub struct VersionPaths {
    pub dir: PathBuf,
    pub pdf: PathBuf,
    pub md: PathBuf,
    /// Parent of the per-converter page caches (`md-pages/<converter>/`).
    pub md_pages: PathBuf,
    pub bib: PathBuf,
    pub abstract_: PathBuf,
    pub meta: PathBuf,
}

pub fn version_paths(root: &Path, id: PaperId, version: &Canonical) -> VersionPaths {
    let dir = version_dir(root, id, version);
    VersionPaths {
        pdf: dir.join(files::PDF),
        md: dir.join(files::MD),
        md_pages: dir.join(files::MD_PAGES),
        bib: dir.join(files::BIB),
        abstract_: dir.join(files::ABSTRACT),
        meta: dir.join(files::META),
        dir,
    }
}

/// Version subdirectories present on disk for `id`, sorted ascending.
pub fn existing_versions(root: &Path, id: PaperId) -> Vec<Canonical> {
    versions_in(&paper_dir(root, id))
}

/// Version subdirectories of a paper directory, sorted ascending.
fn versions_in(paper_dir: &Path) -> Vec<Canonical> {
    let mut out: Vec<Canonical> = match std::fs::read_dir(paper_dir) {
        Ok(rd) => rd
            .flatten()
            .filter_map(|e| e.file_name().to_str()?.parse().ok())
            .collect(),
        Err(_) => Vec::new(),
    };
    out.sort_unstable();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_paths() {
        let root = Path::new("/c");
        let id = PaperId {
            year: 2024,
            num: 463,
        };
        let v: Canonical = "20240319T143540Z".parse().unwrap();
        let p = version_paths(root, id, &v);
        assert_eq!(p.dir, Path::new("/c/2024/0463/20240319T143540Z"));
        assert_eq!(
            p.md_pages,
            Path::new("/c/2024/0463/20240319T143540Z/md-pages")
        );
        assert_eq!(
            paper_meta_path(root, id),
            Path::new("/c/2024/0463/meta.json")
        );
        assert_eq!(models_dir(root), Path::new("/c/models"));
    }

    #[test]
    fn versions_are_sorted_and_non_versions_ignored() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["20250106T174348Z", "meta.json", "20240319T143540Z", "junk"] {
            std::fs::create_dir(dir.path().join(name)).unwrap();
        }
        let got: Vec<String> = versions_in(dir.path())
            .iter()
            .map(|v| v.to_string())
            .collect();
        assert_eq!(got, ["20240319T143540Z", "20250106T174348Z"]);
    }
}
