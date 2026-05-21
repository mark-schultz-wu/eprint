//! Cache layout for fetched ePrint papers.
//!
//! Each paper occupies a directory under the cache root:
//! ```text
//! <cache_root>/
//!   2024/
//!     0463/
//!       paper.pdf
//!       paper.md          # converted (text or ML tier)
//!       paper.bib
//!       abstract.txt
//!       meta.json         # provenance, MinerU version used, timestamps
//! ```

use crate::id::PaperId;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Names of the per-paper artifact files in the cache.
pub mod files {
    pub const PDF: &str = "paper.pdf";
    pub const MD: &str = "paper.md";
    pub const BIB: &str = "paper.bib";
    pub const ABSTRACT: &str = "abstract.txt";
    pub const META: &str = "meta.json";
}

/// On-disk per-paper provenance.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Meta {
    /// MinerU version used to produce the cached `.md`, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mineru_version: Option<String>,
    /// Quality tier the cached `.md` represents ("text" or "ml").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub md_quality: Option<String>,
    /// Unix timestamp of last successful fetch of any artifact.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fetched_unix_s: Option<i64>,
}

/// Directory under the cache root for a given paper.
pub fn dir_for(root: &Path, id: PaperId) -> PathBuf {
    root.join(id.cache_subdir())
}

/// Convenience constructor for the most common per-paper paths.
pub fn paths_for(root: &Path, id: PaperId) -> Paths {
    let dir = dir_for(root, id);
    Paths {
        pdf: dir.join(files::PDF),
        md: dir.join(files::MD),
        bib: dir.join(files::BIB),
        abstract_: dir.join(files::ABSTRACT),
        meta: dir.join(files::META),
        dir,
    }
}

pub struct Paths {
    pub dir: PathBuf,
    pub pdf: PathBuf,
    pub md: PathBuf,
    pub bib: PathBuf,
    pub abstract_: PathBuf,
    pub meta: PathBuf,
}
