//! Where eprint lives: every URL the tool requests is built here.
//!
//! The base defaults to <https://eprint.iacr.org> and can be overridden with
//! `EPRINT_BASE_URL` (a mirror, or the local fake server the integration
//! tests run against).

use crate::ids::version::Canonical;
use crate::ids::PaperId;

pub const DEFAULT_BASE_URL: &str = "https://eprint.iacr.org";

#[derive(Debug, Clone)]
pub struct Site {
    base: String,
}

impl Site {
    pub fn new(base: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_owned(),
        }
    }

    /// The paper's landing page (title, abstract, BibTeX).
    pub fn landing_url(&self, id: PaperId) -> String {
        format!("{}/{}", self.base, id.canonical())
    }

    /// The listing of all of a paper's versions.
    pub fn archive_url(&self, id: PaperId) -> String {
        format!("{}/archive/versions/{}", self.base, id.canonical())
    }

    /// One exact version's PDF: `/archive/<year>/<num>/<unix-seconds>.pdf`.
    /// (This serves the current version too.)
    pub fn version_pdf_url(&self, id: PaperId, version: &Canonical) -> String {
        format!(
            "{}/archive/{}/{}.pdf",
            self.base,
            id.canonical(),
            version.to_unix()
        )
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
        Self::new(DEFAULT_BASE_URL)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_pad_paper_numbers_to_three_digits() {
        let site = Site::default();
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
        let big = PaperId {
            year: 2024,
            num: 1234,
        };
        assert_eq!(site.landing_url(big), "https://eprint.iacr.org/2024/1234");
    }

    #[test]
    fn base_url_is_configurable_and_trailing_slash_tolerant() {
        let site = Site::new("http://127.0.0.1:8080/");
        assert_eq!(site.oai_url(), "http://127.0.0.1:8080/oai");
        assert_eq!(site.rss_url(), "http://127.0.0.1:8080/rss/rss.xml");
    }
}
