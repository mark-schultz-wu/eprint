//! Where eprint lives: every URL the tool requests is built here.
//!
//! The base defaults to the mirror <https://eprint.iacr.asia> and can be
//! overridden with `EPRINT_BASE_URL` (e.g. <https://eprint.iacr.org>, another
//! mirror, or the local fake server the integration tests run against).
//!
//! Hosts differ in one capability: the **version archive**
//! (`/archive/versions/<id>` and per-version PDFs). eprint.iacr.org has it;
//! the mirror does not, and serves only each paper's current PDF at
//! `/<year>/<num>.pdf`. [`Site::has_archive`] records which kind of host
//! this is; see [`crate::config`] for how it's chosen.

use crate::ids::version::Canonical;
use crate::ids::PaperId;

/// The default host: a mirror of eprint.iacr.org that serves landing pages,
/// current PDFs, OAI-PMH and RSS, but no version archive.
pub const DEFAULT_BASE_URL: &str = "https://eprint.iacr.asia";

/// The primary eprint host, which also serves the version archive.
pub const IACR_BASE_URL: &str = "https://eprint.iacr.org";

#[derive(Debug, Clone)]
pub struct Site {
    base: String,
    archive: bool,
}

impl Site {
    /// `archive`: whether this host serves the version archive.
    pub fn new(base: &str, archive: bool) -> Self {
        Self {
            base: base.trim_end_matches('/').to_owned(),
            archive,
        }
    }

    /// Whether the host serves `/archive/versions/<id>` and per-version PDFs.
    /// Without it, versions come from OAI-PMH and only the current PDF can be
    /// fetched.
    pub fn has_archive(&self) -> bool {
        self.archive
    }

    /// The paper's landing page (title, abstract, BibTeX).
    pub fn landing_url(&self, id: PaperId) -> String {
        format!("{}/{}", self.base, id.canonical())
    }

    /// The listing of all of a paper's versions (archive hosts only).
    pub fn archive_url(&self, id: PaperId) -> String {
        format!("{}/archive/versions/{}", self.base, id.canonical())
    }

    /// One exact version's PDF: `/archive/<year>/<num>/<unix-seconds>.pdf`
    /// (archive hosts only). This serves the current version too.
    pub fn version_pdf_url(&self, id: PaperId, version: &Canonical) -> String {
        format!(
            "{}/archive/{}/{}.pdf",
            self.base,
            id.canonical(),
            version.to_unix()
        )
    }

    /// The paper's current PDF: `/<year>/<num>.pdf`. Every host serves it,
    /// but it is whatever version is current *now*.
    pub fn current_pdf_url(&self, id: PaperId) -> String {
        format!("{}/{}.pdf", self.base, id.canonical())
    }

    /// The OAI-PMH endpoint.
    pub fn oai_url(&self) -> String {
        format!("{}/oai", self.base)
    }

    /// The RSS feed.
    pub fn rss_url(&self) -> String {
        format!("{}/rss/rss.xml", self.base)
    }
}

impl Default for Site {
    fn default() -> Self {
        Self::new(DEFAULT_BASE_URL, false)
    }
}

/// Whether `base` is the default mirror (which has no version archive).
pub fn is_default_mirror(base: &str) -> bool {
    base.trim_end_matches('/') == DEFAULT_BASE_URL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_pad_paper_numbers_to_three_digits() {
        let site = Site::new(IACR_BASE_URL, true);
        let id = PaperId {
            year: 2020,
            num: 18,
        };
        assert_eq!(site.landing_url(id), "https://eprint.iacr.org/2020/018");
        assert_eq!(
            site.archive_url(id),
            "https://eprint.iacr.org/archive/versions/2020/018"
        );
        let v: Canonical = "20200110T000000Z".parse().unwrap();
        assert_eq!(
            site.version_pdf_url(id, &v),
            "https://eprint.iacr.org/archive/2020/018/1578614400.pdf"
        );
        assert_eq!(
            site.current_pdf_url(id),
            "https://eprint.iacr.org/2020/018.pdf"
        );
        let big = PaperId {
            year: 2024,
            num: 1234,
        };
        assert_eq!(site.landing_url(big), "https://eprint.iacr.org/2024/1234");
    }

    #[test]
    fn base_url_is_configurable_and_trailing_slash_tolerant() {
        let site = Site::new("http://127.0.0.1:8080/", true);
        assert_eq!(site.oai_url(), "http://127.0.0.1:8080/oai");
        assert_eq!(site.rss_url(), "http://127.0.0.1:8080/rss/rss.xml");
    }

    #[test]
    fn the_default_is_the_mirror_without_an_archive() {
        let site = Site::default();
        assert!(!site.has_archive());
        let id = PaperId {
            year: 2023,
            num: 237,
        };
        assert_eq!(site.landing_url(id), "https://eprint.iacr.asia/2023/237");
        assert_eq!(
            site.current_pdf_url(id),
            "https://eprint.iacr.asia/2023/237.pdf"
        );
        assert!(is_default_mirror("https://eprint.iacr.asia/"));
        assert!(!is_default_mirror(IACR_BASE_URL));
    }
}
