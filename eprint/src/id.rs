//! Parse and format ePrint paper IDs (YEAR/NUM).

use std::fmt;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PaperId {
    pub year: u16,
    pub num: u32,
}

impl PaperId {
    /// PDF download URL.
    pub fn pdf_url(&self) -> String {
        format!("https://eprint.iacr.org/{}/{}.pdf", self.year, self.num)
    }

    /// HTML landing page URL.
    pub fn html_url(&self) -> String {
        format!("https://eprint.iacr.org/{}/{}", self.year, self.num)
    }

    /// Canonical "year/num" form.
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
        for pat in patterns() {
            if let Some(caps) = pat.captures(s) {
                let year: u16 = caps.name("year").unwrap().as_str().parse().map_err(|_| ParseError(s.into()))?;
                let num: u32 = caps.name("num").unwrap().as_str().parse().map_err(|_| ParseError(s.into()))?;
                return Ok(PaperId { year, num });
            }
        }
        Err(ParseError(s.into()))
    }
}

fn patterns() -> &'static [regex::Regex] {
    static PATS: OnceLock<Vec<regex::Regex>> = OnceLock::new();
    PATS.get_or_init(|| {
        vec![
            regex::Regex::new(r"^(?P<year>\d{4})[/_-](?P<num>\d{1,6})$").unwrap(),
            regex::Regex::new(r"^(?i)eprint[-_/](?P<year>\d{4})[/_-](?P<num>\d{1,6})$").unwrap(),
            regex::Regex::new(r"^(?i)https?://eprint\.iacr\.org/(?P<year>\d{4})/(?P<num>\d{1,6})(?:\.pdf)?/?$").unwrap(),
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_canonical() {
        let id: PaperId = "2024/463".parse().unwrap();
        assert_eq!(id.year, 2024);
        assert_eq!(id.num, 463);
    }

    #[test]
    fn parses_dashes_and_urls() {
        let cases = [
            "2024-463",
            "2024_463",
            "https://eprint.iacr.org/2024/463",
            "https://eprint.iacr.org/2024/463.pdf",
            "eprint-2024-463",
        ];
        for c in cases {
            let id: PaperId = c.parse().unwrap_or_else(|_| panic!("failed: {c}"));
            assert_eq!(id.year, 2024);
            assert_eq!(id.num, 463);
        }
    }

    #[test]
    fn pads_cache_subdir() {
        let id = PaperId { year: 2025, num: 7 };
        assert_eq!(id.cache_subdir(), "2025/0007");
    }

    #[test]
    fn rejects_garbage() {
        assert!("not a paper".parse::<PaperId>().is_err());
        assert!("2024/".parse::<PaperId>().is_err());
    }
}
