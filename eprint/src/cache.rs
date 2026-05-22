//! Per-paper cache layout, keyed on eprint's version timestamps.
//!
//! ```text
//! <cache_root>/
//!   2024/
//!     0463/
//!       meta.json                  # PaperMeta
//!       20250106T174348Z/          # one dir per known version
//!         meta.json                # VersionMeta
//!         paper.pdf
//!         paper.md
//!         paper.bib
//!         abstract.txt
//!       20240319T143540Z/
//!         ...
//! ```
//!
//! Version directory names use the canonical form from `crate::version`
//! (filesystem-friendly basic ISO 8601 UTC).

use crate::id::PaperId;
use crate::version;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub mod files {
    pub const PDF: &str = "paper.pdf";
    pub const MD: &str = "paper.md";
    pub const BIB: &str = "paper.bib";
    pub const ABSTRACT: &str = "abstract.txt";
    pub const VERSION_META: &str = "meta.json";
    pub const PAPER_META: &str = "meta.json";
}

/// Magic field embedded in every paper-level `meta.json` so destructive
/// operations can positively identify our cache entries.
pub const TOOL_TAG: &str = "eprint";

/// Paper-level state. Lives at `<root>/<year>/<num>/meta.json`. Has no
/// `Default` impl by design — see [`PaperMeta::for_first_fetch`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperMeta {
    /// Tool identifier; always `"eprint"`.
    pub tool: String,
    /// Canonical timestamp of the version the tool treats as current.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_version: Option<version::Canonical>,
    /// All version timestamps the tool knows about (cached or not),
    /// ascending order. Populated from archive scrape + augmented by sync.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub known_versions: Vec<version::Canonical>,
    /// Paper title from the landing page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

impl PaperMeta {
    /// Construct a paper-level meta for a brand-new fetch, where we're
    /// about to write `<version>/` for this paper for the first time.
    pub fn for_first_fetch(version: version::Canonical) -> Self {
        Self {
            tool: TOOL_TAG.into(),
            current_version: Some(version.clone()),
            known_versions: vec![version],
            title: None,
        }
    }
}

/// Per-version state. Lives at `<root>/<year>/<num>/<version>/meta.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VersionMeta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fetched_unix_s: Option<i64>,
    /// "text" or "ml"; `None` if `paper.md` hasn't been generated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub md_quality: Option<String>,
    /// MinerU version used to produce `paper.md`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mineru_version: Option<String>,
}

pub fn paper_dir(root: &Path, id: PaperId) -> PathBuf {
    root.join(id.cache_subdir())
}

pub fn version_dir(root: &Path, id: PaperId, version: &version::Canonical) -> PathBuf {
    paper_dir(root, id).join(version.as_str())
}

pub fn paper_meta_path(root: &Path, id: PaperId) -> PathBuf {
    paper_dir(root, id).join(files::PAPER_META)
}

pub struct VersionPaths {
    pub dir: PathBuf,
    pub pdf: PathBuf,
    pub md: PathBuf,
    pub bib: PathBuf,
    pub abstract_: PathBuf,
    pub meta: PathBuf,
}

pub fn version_paths(root: &Path, id: PaperId, version: &version::Canonical) -> VersionPaths {
    let dir = version_dir(root, id, version);
    VersionPaths {
        pdf: dir.join(files::PDF),
        md: dir.join(files::MD),
        bib: dir.join(files::BIB),
        abstract_: dir.join(files::ABSTRACT),
        meta: dir.join(files::VERSION_META),
        dir,
    }
}

pub async fn read_paper_meta(root: &Path, id: PaperId) -> Option<PaperMeta> {
    let path = paper_meta_path(root, id);
    let s = tokio::fs::read_to_string(&path).await.ok()?;
    serde_json::from_str(&s).ok()
}

pub async fn write_paper_meta(root: &Path, id: PaperId, meta: &PaperMeta) -> std::io::Result<()> {
    let dir = paper_dir(root, id);
    tokio::fs::create_dir_all(&dir).await?;
    let path = paper_meta_path(root, id);
    let bytes = serde_json::to_vec_pretty(meta)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    tokio::fs::write(path, bytes).await
}

pub async fn read_version_meta(
    root: &Path,
    id: PaperId,
    version: &version::Canonical,
) -> VersionMeta {
    let path = version_paths(root, id, version).meta;
    match tokio::fs::read_to_string(&path).await {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => VersionMeta::default(),
    }
}

pub async fn write_version_meta(
    root: &Path,
    id: PaperId,
    version: &version::Canonical,
    meta: &VersionMeta,
) -> std::io::Result<()> {
    let paths = version_paths(root, id, version);
    tokio::fs::create_dir_all(&paths.dir).await?;
    let bytes = serde_json::to_vec_pretty(meta)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    tokio::fs::write(paths.meta, bytes).await
}

/// Enumerate version subdirectories present on disk, sorted ascending.
pub fn existing_versions(root: &Path, id: PaperId) -> Vec<version::Canonical> {
    let dir = paper_dir(root, id);
    let mut out: Vec<version::Canonical> = match std::fs::read_dir(&dir) {
        Ok(rd) => rd
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name.parse::<version::Canonical>().ok()
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    out.sort_unstable();
    out
}
