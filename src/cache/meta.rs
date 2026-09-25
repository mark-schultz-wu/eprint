//! Metadata files: paper-level and version-level `meta.json`, and the
//! last-sync stamp.

use super::{last_sync_path, paper_dir, paper_meta_path, version_paths, TOOL_TAG};
use crate::ids::version::Canonical;
use crate::ids::PaperId;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::Path;

/// Version of the paper-cache format. Papers cached under an older schema
/// are purged on next use ([`purge_if_outdated`]). History:
///
/// - 0 (no field): `sync` filed OAI datestamps as versions and never
///   advanced `current_version`, so the latest PDF could be saved under an
///   old version's directory.
/// - 2: versions come only from the archive listing; `sync` records a
///   `last_modified` hint that triggers a re-listing.
pub const SCHEMA: u32 = 2;

/// Paper-level state. Lives at `<root>/<year>/<num>/meta.json`. Has no
/// `Default` impl by design: construct it with [`PaperMeta::new`] so the
/// [`TOOL_TAG`] and [`SCHEMA`] are always set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperMeta {
    /// Tool identifier; always [`TOOL_TAG`].
    pub tool: String,
    /// Cache format version; see [`SCHEMA`].
    #[serde(default)]
    pub schema: u32,
    /// The paper's newest version, per the last archive listing (or, if the
    /// listing was unreachable, OAI-PMH).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_version: Option<Canonical>,
    /// All version timestamps known from the archive listing (cached or
    /// not), ascending.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub known_versions: Vec<Canonical>,
    /// Newest OAI-PMH datestamp `sync` has seen for this paper. That's the
    /// paper's last modification: usually a new version, but possibly a
    /// metadata-only edit, so it's a hint to re-list, never a version itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<Canonical>,
    /// How far the last archive listing accounts for: the newer of the
    /// listed current version and `last_modified` at listing time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub listed_through: Option<Canonical>,
    /// Paper title from the landing page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

impl PaperMeta {
    pub fn new(current_version: Option<Canonical>, known_versions: Vec<Canonical>) -> Self {
        Self {
            tool: TOOL_TAG.into(),
            schema: SCHEMA,
            current_version,
            known_versions,
            last_modified: None,
            listed_through: None,
            title: None,
        }
    }

    /// Whether the version list must be (re)fetched from the archive: we
    /// have none, or `sync` saw a modification after the last listing.
    pub fn needs_listing(&self) -> bool {
        if self.known_versions.is_empty() || self.current_version.is_none() {
            return true;
        }
        let checked = self.listed_through.or(self.current_version);
        self.last_modified > checked
    }

    /// Replace the version list with a fresh archive listing.
    pub fn record_listing(&mut self, known_versions: Vec<Canonical>, current: Option<Canonical>) {
        self.current_version = current
            .or_else(|| known_versions.last().copied())
            .or(self.current_version);
        self.known_versions = known_versions;
        self.listed_through = self.current_version.max(self.last_modified);
    }

    /// Note an OAI-PMH datestamp from `sync`. Returns whether it was news.
    pub fn note_modified(&mut self, datestamp: Canonical) -> bool {
        if self.last_modified >= Some(datestamp) {
            return false;
        }
        self.last_modified = Some(datestamp);
        true
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

/// Delete a paper cached under an older [`SCHEMA`] so it's re-fetched from
/// scratch. Returns whether anything was purged.
pub async fn purge_if_outdated(root: &Path, id: PaperId) -> io::Result<bool> {
    match read_paper_meta(root, id).await {
        Some(meta) if meta.schema < SCHEMA => {
            tokio::fs::remove_dir_all(paper_dir(root, id)).await?;
            Ok(true)
        }
        _ => Ok(false),
    }
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

    fn v(s: &str) -> Canonical {
        s.parse().unwrap()
    }

    #[test]
    fn listing_is_needed_until_versions_are_known() {
        assert!(PaperMeta::new(None, vec![]).needs_listing());
        assert!(PaperMeta::new(None, vec![v("20240319T143540Z")]).needs_listing());
        let mut m = PaperMeta::new(None, vec![]);
        m.record_listing(vec![v("20240319T143540Z"), v("20241017T150428Z")], None);
        assert_eq!(
            m.current_version,
            Some(v("20241017T150428Z")),
            "newest wins without a marker"
        );
        assert!(!m.needs_listing());
    }

    /// Regression: `sync` learning of a newer revision must trigger a
    /// re-listing, which advances `current_version`. (Previously the old
    /// current version stuck, and the latest PDF was saved under it.)
    #[test]
    fn a_newer_modification_triggers_relisting_that_advances_current() {
        let mut m = PaperMeta::new(None, vec![]);
        m.record_listing(vec![v("20241017T150428Z")], Some(v("20241017T150428Z")));
        assert!(m.note_modified(v("20250106T174348Z")));
        assert!(m.needs_listing());
        m.record_listing(
            vec![v("20241017T150428Z"), v("20250106T174348Z")],
            Some(v("20250106T174348Z")),
        );
        assert_eq!(m.current_version, Some(v("20250106T174348Z")));
        assert!(!m.needs_listing());
    }

    /// A metadata-only edit bumps the OAI datestamp without a new version:
    /// one re-listing, then quiet.
    #[test]
    fn a_metadata_only_edit_relists_once() {
        let mut m = PaperMeta::new(None, vec![]);
        m.record_listing(vec![v("20241017T150428Z")], Some(v("20241017T150428Z")));
        m.note_modified(v("20250301T000000Z"));
        assert!(m.needs_listing());
        m.record_listing(vec![v("20241017T150428Z")], Some(v("20241017T150428Z")));
        assert_eq!(m.current_version, Some(v("20241017T150428Z")));
        assert!(!m.needs_listing(), "listing accounted for the edit");
        assert!(
            !m.note_modified(v("20250301T000000Z")),
            "same datestamp again isn't news"
        );
        assert!(!m.needs_listing());
    }

    #[tokio::test]
    async fn outdated_schema_is_purged_current_is_kept() {
        let root = tempfile::tempdir().unwrap();
        let old = PaperId {
            year: 2024,
            num: 463,
        };
        let dir = paper_dir(root.path(), old);
        std::fs::create_dir_all(dir.join("20241017T150428Z")).unwrap();
        std::fs::write(
            dir.join("meta.json"),
            r#"{"tool":"eprint","current_version":"20241017T150428Z"}"#,
        )
        .unwrap();
        assert!(purge_if_outdated(root.path(), old).await.unwrap());
        assert!(!dir.exists());

        let new = PaperId { year: 2024, num: 1 };
        write_paper_meta(
            root.path(),
            new,
            &PaperMeta::for_first_fetch(v("20240319T143540Z")),
        )
        .await
        .unwrap();
        assert!(!purge_if_outdated(root.path(), new).await.unwrap());
        assert!(
            !purge_if_outdated(root.path(), PaperId { year: 2020, num: 2 })
                .await
                .unwrap()
        );
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
