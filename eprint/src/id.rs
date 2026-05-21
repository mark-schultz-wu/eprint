//! Parse and format ePrint paper IDs and version-qualified references.
//!
//! - `PaperId` is just `(year, num)`.
//! - `PaperRef` is `PaperId` with an optional version suffix
//!   (`2024/463@v2` → version = Some(2); `2024/463` → version = None,
//!   meaning "current").

use std::fmt;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PaperId {
    pub year: u16,
    pub num: u32,
}

impl PaperId {
    pub fn pdf_url(&self) -> String {
        format!("https://eprint.iacr.org/{}/{}.pdf", self.year, self.num)
    }

    pub fn html_url(&self) -> String {
        format!("https://eprint.iacr.org/{}/{}", self.year, self.num)
    }

    pub fn canonical(&self) -> String {
        format!("{}/{}", self.year, self.num)
    }

    /// Subdirectory under the cache root: `<year>/<num:04>/`.
    pub fn cache_subdir(&self) -> String {
        format!("{}/{:04}", self.year, self.num)
    }
}

impl fmt::Display for PaperId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.canonical())
    }
}

/// Reference to a specific paper, optionally pinned to a version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaperRef {
    pub id: PaperId,
    /// `None` = "current version" (whichever vN is marked current in the
    /// paper's `meta.json`). `Some(N)` = `vN` explicitly.
    pub version: Option<u32>,
}

impl PaperRef {
    pub fn current(id: PaperId) -> Self {
        Self { id, version: None }
    }
}

impl fmt::Display for PaperRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.version {
            Some(v) => write!(f, "{}@v{}", self.id, v),
            None => self.id.fmt(f),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error(
    "unrecognized eprint reference {0:?}; expected forms like '2024/463', \
     '2024-463', '2024/463@v2', or a full eprint.iacr.org URL"
)]
pub struct ParseError(String);

impl std::str::FromStr for PaperId {
    type Err = ParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        for pat in id_patterns() {
            if let Some(caps) = pat.captures(s) {
                let year: u16 = caps["year"].parse().map_err(|_| ParseError(s.into()))?;
                let num: u32 = caps["num"].parse().map_err(|_| ParseError(s.into()))?;
                return Ok(PaperId { year, num });
            }
        }
        Err(ParseError(s.into()))
    }
}

impl std::str::FromStr for PaperRef {
    type Err = ParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        // Split on @ once; left side parses as PaperId, right side as `vN`.
        if let Some((base, suffix)) = s.split_once('@') {
            let id: PaperId = base.parse()?;
            let v = suffix
                .strip_prefix('v')
                .and_then(|n| n.parse::<u32>().ok())
                .ok_or_else(|| ParseError(s.into()))?;
            Ok(PaperRef { id, version: Some(v) })
        } else {
            Ok(PaperRef::current(s.parse()?))
        }
    }
}

fn id_patterns() -> &'static [regex::Regex] {
    static PATS: OnceLock<Vec<regex::Regex>> = OnceLock::new();
    PATS.get_or_init(|| {
        vec![
            regex::Regex::new(r"^(?P<year>\d{4})[/_-](?P<num>\d{1,6})$").unwrap(),
            regex::Regex::new(r"^(?i)eprint[-_/](?P<year>\d{4})[/_-](?P<num>\d{1,6})$").unwrap(),
            regex::Regex::new(
                r"^(?i)https?://eprint\.iacr\.org/(?P<year>\d{4})/(?P<num>\d{1,6})(?:\.pdf)?/?$",
            )
            .unwrap(),
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_canonical_id() {
        let id: PaperId = "2024/463".parse().unwrap();
        assert_eq!(id.year, 2024);
        assert_eq!(id.num, 463);
    }

    #[test]
    fn parses_ref_without_version() {
        let r: PaperRef = "2024/463".parse().unwrap();
        assert_eq!(r.id.year, 2024);
        assert_eq!(r.version, None);
    }

    #[test]
    fn parses_ref_with_version() {
        let r: PaperRef = "2024/463@v3".parse().unwrap();
        assert_eq!(r.id.num, 463);
        assert_eq!(r.version, Some(3));
    }

    #[test]
    fn rejects_bad_version_suffix() {
        assert!("2024/463@".parse::<PaperRef>().is_err());
        assert!("2024/463@v".parse::<PaperRef>().is_err());
        assert!("2024/463@vfoo".parse::<PaperRef>().is_err());
        assert!("2024/463@2".parse::<PaperRef>().is_err()); // missing 'v'
    }

    #[test]
    fn parses_dashes_and_urls() {
        let cases = [
            ("2024-463", None),
            ("2024_463", None),
            ("https://eprint.iacr.org/2024/463", None),
            ("https://eprint.iacr.org/2024/463.pdf", None),
            ("eprint-2024-463", None),
        ];
        for (s, expected_v) in cases {
            let r: PaperRef = s.parse().unwrap_or_else(|_| panic!("failed: {s}"));
            assert_eq!(r.id.year, 2024);
            assert_eq!(r.id.num, 463);
            assert_eq!(r.version, expected_v);
        }
    }

    #[test]
    fn pads_cache_subdir() {
        let id = PaperId { year: 2025, num: 7 };
        assert_eq!(id.cache_subdir(), "2025/0007");
    }

    #[test]
    fn display_roundtrips_with_version() {
        let r = PaperRef { id: PaperId { year: 2024, num: 463 }, version: Some(2) };
        assert_eq!(r.to_string(), "2024/463@v2");
    }
}
