//! Versioned per-paper cache layout.
//!
//! ```text
//! <cache_root>/
//!   2024/
//!     0463/
//!       meta.json                  # PaperMeta
//!       v1/
//!         meta.json                # VersionMeta
//!         paper.pdf
//!         paper.md                 # written by `eprint convert`
//!         paper.bib
//!         abstract.txt
//!       v2/                        # added when sync sees a newer OAI datestamp
//!         ...
//! ```
//!
//! Versions are integer-numbered. The paper-level `meta.json` records
//! which version is *current* and the most recent OAI-PMH datestamp the
//! tool has seen for this paper (filled by sync, not by fetch).

use crate::id::PaperId;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// File names within a `vN/` directory.
pub mod files {
    pub const PDF: &str = "paper.pdf";
    pub const MD: &str = "paper.md";
    pub const BIB: &str = "paper.bib";
    pub const ABSTRACT: &str = "abstract.txt";
    pub const VERSION_META: &str = "meta.json";
    pub const PAPER_META: &str = "meta.json";
}

/// Magic field embedded in every paper-level `meta.json` so destructive
/// operations (e.g. `eprint cache clear`) can positively identify that a
/// directory is one of our cache entries before recursively deleting it.
pub const TOOL_TAG: &str = "eprint";

/// Paper-level state. Lives at `<root>/<year>/<num>/meta.json`.
///
/// No `Default` impl: a `PaperMeta` should never exist except to describe
/// an actually-fetched paper. Use [`PaperMeta::for_first_fetch`] when you
/// know you're about to write `v1/` for a paper that wasn't on disk yet.
/// For "read existing or report absent," use [`read_paper_meta`], which
/// returns `Option<PaperMeta>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperMeta {
    /// Tool that owns this directory; always `"eprint"` for us.
    pub tool: String,
    /// Which `vN` is the user-visible "latest" version. Never `None` for
    /// a meta that's been written to disk by our code — kept as `Option`
    /// so deserialisation of legacy / partially-written meta.json files
    /// doesn't error out hard.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_version: Option<u32>,
    /// Most recent OAI-PMH datestamp seen by `eprint sync` (ISO 8601).
    /// `None` if sync has never observed this paper.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_known_oai_datestamp: Option<String>,
    /// Paper title from the landing page (best-effort).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

impl PaperMeta {
    /// Construct a paper-level meta for a brand-new fetch, where we're
    /// about to write `v{version}/` for this paper for the first time.
    /// All other fields start as their natural empty state.
    pub fn for_first_fetch(version: u32) -> Self {
        Self {
            tool: TOOL_TAG.into(),
            current_version: Some(version),
            latest_known_oai_datestamp: None,
            title: None,
        }
    }
}

/// Per-version state. Lives at `<root>/<year>/<num>/vN/meta.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VersionMeta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fetched_unix_s: Option<i64>,
    /// OAI-PMH datestamp this version corresponds to (filled by sync when
    /// the version is first observed; null for versions that were created
    /// by direct fetch without ever being seen by sync).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oai_datestamp: Option<String>,
    /// "text" or "ml"; `None` if `paper.md` hasn't been generated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub md_quality: Option<String>,
    /// MinerU version used to produce `paper.md`, only set when md_quality == "ml".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mineru_version: Option<String>,
}

/// `<root>/<year>/<num>/`.
pub fn paper_dir(root: &Path, id: PaperId) -> PathBuf {
    root.join(id.cache_subdir())
}

/// `<root>/<year>/<num>/vN/`.
pub fn version_dir(root: &Path, id: PaperId, version: u32) -> PathBuf {
    paper_dir(root, id).join(format!("v{version}"))
}

/// `<root>/<year>/<num>/meta.json`.
pub fn paper_meta_path(root: &Path, id: PaperId) -> PathBuf {
    paper_dir(root, id).join(files::PAPER_META)
}

/// All on-disk paths inside one version directory.
pub struct VersionPaths {
    pub dir: PathBuf,
    pub pdf: PathBuf,
    pub md: PathBuf,
    pub bib: PathBuf,
    pub abstract_: PathBuf,
    pub meta: PathBuf,
}

pub fn version_paths(root: &Path, id: PaperId, version: u32) -> VersionPaths {
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

/// Read the paper-level `meta.json`, returning `None` if the file is
/// absent or unparseable. Callers must decide what to do when there's no
/// existing meta — historically we silently defaulted, which let a `tool`
/// tag leak onto unrelated directories.
pub async fn read_paper_meta(root: &Path, id: PaperId) -> Option<PaperMeta> {
    let path = paper_meta_path(root, id);
    let s = tokio::fs::read_to_string(&path).await.ok()?;
    serde_json::from_str(&s).ok()
}

/// Write the paper-level `meta.json` atomically.
pub async fn write_paper_meta(root: &Path, id: PaperId, meta: &PaperMeta) -> std::io::Result<()> {
    let dir = paper_dir(root, id);
    tokio::fs::create_dir_all(&dir).await?;
    let path = paper_meta_path(root, id);
    let bytes = serde_json::to_vec_pretty(meta)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    tokio::fs::write(path, bytes).await
}

pub async fn read_version_meta(root: &Path, id: PaperId, version: u32) -> VersionMeta {
    let path = version_paths(root, id, version).meta;
    match tokio::fs::read_to_string(&path).await {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => VersionMeta::default(),
    }
}

pub async fn write_version_meta(
    root: &Path,
    id: PaperId,
    version: u32,
    meta: &VersionMeta,
) -> std::io::Result<()> {
    let paths = version_paths(root, id, version);
    tokio::fs::create_dir_all(&paths.dir).await?;
    let bytes = serde_json::to_vec_pretty(meta)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    tokio::fs::write(paths.meta, bytes).await
}

/// Enumerate `vN` subdirectories present on disk, sorted ascending by N.
pub fn existing_versions(root: &Path, id: PaperId) -> Vec<u32> {
    let dir = paper_dir(root, id);
    let mut out: Vec<u32> = match std::fs::read_dir(&dir) {
        Ok(rd) => rd
            .flatten()
            .filter_map(|e| {
                let name = e.file_name();
                let s = name.to_str()?;
                s.strip_prefix('v').and_then(|n| n.parse::<u32>().ok())
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    out.sort_unstable();
    out
}

