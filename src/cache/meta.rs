//! Metadata files: paper-level and version-level `meta.json`, and the
//! last-sync stamp.

use super::{last_sync_path, paper_dir, paper_meta_path, version_paths, TOOL_TAG};
use crate::ids::version::Canonical;
use crate::ids::PaperId;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::Path;

/// Paper-level state. Lives at `<root>/<year>/<num>/meta.json`. Has no
/// `Default` impl by design: construct it with [`PaperMeta::new`] so the
/// [`TOOL_TAG`] is always set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperMeta {
    /// Tool identifier; always [`TOOL_TAG`].
    pub tool: String,
    /// Canonical timestamp of the version the tool treats as current.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_version: Option<Canonical>,
    /// All version timestamps the tool knows about (cached or not),
    /// ascending order. Populated from archive scrape + augmented by sync.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub known_versions: Vec<Canonical>,
    /// Paper title from the landing page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

impl PaperMeta {
    pub fn new(current_version: Option<Canonical>, known_versions: Vec<Canonical>) -> Self {
        Self {
            tool: TOOL_TAG.into(),
            current_version,
            known_versions,
            title: None,
        }
    }

    /// Meta for a brand-new fetch, where we're about to write `<version>/`
    /// for this paper for the first time.
    pub fn for_first_fetch(version: Canonical) -> Self {
        Self::new(Some(version), vec![version])
    }
}

/// Per-version state. Lives at `<root>/<year>/<num>/<version>/meta.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VersionMeta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fetched_unix_s: Option<i64>,
    /// [`crate::markdown::CONVERTER_ID`] of the converter that produced
    /// `paper.md`; `None` if it hasn't been generated. (Older caches carried
    /// `md_quality` / `mineru_version` instead; serde ignores those, so such
    /// Markdown reads as stale and is regenerated.)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub md_converter: Option<String>,
}

pub async fn read_paper_meta(root: &Path, id: PaperId) -> Option<PaperMeta> {
    let s = tokio::fs::read_to_string(paper_meta_path(root, id))
        .await
        .ok()?;
    serde_json::from_str(&s).ok()
}

pub async fn write_paper_meta(root: &Path, id: PaperId, meta: &PaperMeta) -> io::Result<()> {
    tokio::fs::create_dir_all(paper_dir(root, id)).await?;
    tokio::fs::write(paper_meta_path(root, id), to_json(meta)?).await
}

/// A missing or unreadable version meta reads as empty.
pub async fn read_version_meta(root: &Path, id: PaperId, version: &Canonical) -> VersionMeta {
    match tokio::fs::read_to_string(version_paths(root, id, version).meta).await {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => VersionMeta::default(),
    }
}

pub async fn write_version_meta(
    root: &Path,
    id: PaperId,
    version: &Canonical,
    meta: &VersionMeta,
) -> io::Result<()> {
    let paths = version_paths(root, id, version);
    tokio::fs::create_dir_all(&paths.dir).await?;
    tokio::fs::write(paths.meta, to_json(meta)?).await
}

/// Unix time `eprint sync` last completed, if it ever has.
pub async fn read_last_sync(root: &Path) -> Option<i64> {
    let s = tokio::fs::read_to_string(last_sync_path(root)).await.ok()?;
    s.trim().parse().ok()
}

pub async fn write_last_sync(root: &Path, unix_s: i64) -> io::Result<()> {
    tokio::fs::create_dir_all(root).await?;
    tokio::fs::write(last_sync_path(root), unix_s.to_string()).await
}

fn to_json(value: &impl Serialize) -> io::Result<Vec<u8>> {
    serde_json::to_vec_pretty(value).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn paper_meta_round_trips_with_tool_tag() {
        let root = tempfile::tempdir().unwrap();
        let id = PaperId {
            year: 2024,
            num: 463,
        };
        let v: Canonical = "20240319T143540Z".parse().unwrap();
        write_paper_meta(root.path(), id, &PaperMeta::for_first_fetch(v))
            .await
            .unwrap();
        let back = read_paper_meta(root.path(), id).await.unwrap();
        assert_eq!(back.tool, TOOL_TAG);
        assert_eq!(back.current_version, Some(v));
        assert_eq!(back.known_versions, vec![v]);
    }

    #[tokio::test]
    async fn missing_version_meta_reads_as_default() {
        let root = tempfile::tempdir().unwrap();
        let v: Canonical = "20240319T143540Z".parse().unwrap();
        let m = read_version_meta(root.path(), PaperId { year: 2024, num: 1 }, &v).await;
        assert!(m.fetched_unix_s.is_none() && m.md_converter.is_none());
    }

    #[tokio::test]
    async fn last_sync_round_trips() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(read_last_sync(root.path()).await, None);
        write_last_sync(root.path(), 1_700_000_000).await.unwrap();
        assert_eq!(read_last_sync(root.path()).await, Some(1_700_000_000));
    }
}
