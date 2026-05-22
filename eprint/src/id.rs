//! Parse ePrint paper IDs.

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

    pub fn archive_url(&self) -> String {
        format!("https://eprint.iacr.org/archive/versions/{}/{}", self.year, self.num)
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

#[derive(Debug, thiserror::Error)]
#[error("unrecognized eprint id {0:?}; expected forms like '2024/463', '2024-463', or a full eprint.iacr.org URL")]
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
    fn parses_alternative_forms() {
        for s in &["2024-463", "2024_463", "https://eprint.iacr.org/2024/463",
                   "https://eprint.iacr.org/2024/463.pdf", "eprint-2024-463"] {
            let id: PaperId = s.parse().unwrap_or_else(|_| panic!("failed: {s}"));
            assert_eq!(id.year, 2024);
            assert_eq!(id.num, 463);
        }
    }

    #[test]
    fn pads_cache_subdir() {
        assert_eq!(PaperId { year: 2025, num: 7 }.cache_subdir(), "2025/0007");
    }

    #[test]
    fn rejects_garbage() {
        assert!("not a paper".parse::<PaperId>().is_err());
        assert!("2024/".parse::<PaperId>().is_err());
        assert!("2024/463@v1".parse::<PaperId>().is_err());
    }
}
