//! Canonical version-timestamp handling.
//!
//! eprint identifies a paper version by its upload timestamp. The site
//! itself uses two notations for the same instant:
//!
//! - **Compact** (archive page listing): `YYYYMMDD:HHMMSS` — e.g. `20240319:143540`
//! - **Extended** (OAI-PMH datestamp): `YYYY-MM-DDThh:mm:ssZ` — e.g. `2024-03-19T14:35:40Z`
//!
//! We adopt a third, filesystem-friendly **canonical** form for storage
//! and CLI display: `YYYYMMDDTHHMMSSZ` (ISO 8601 basic UTC). It strips
//! the colon (Windows-incompatible) and keeps the `Z` (signals UTC).
//! Converters here flow eprint's two formats into the canonical form.

use std::sync::OnceLock;

/// Canonical version-timestamp shape: 16 chars, e.g. `20240319T143540Z`.
pub const CANONICAL_REGEX: &str = r"^\d{8}T\d{6}Z$";

#[derive(Debug, thiserror::Error)]
pub enum VersionError {
    #[error("malformed version timestamp {got:?}; expected YYYYMMDDTHHMMSSZ or convertible eprint form")]
    Shape { got: String },
}

/// Returns true if `s` matches [`CANONICAL_REGEX`].
pub fn is_canonical(s: &str) -> bool {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(CANONICAL_REGEX).unwrap())
        .is_match(s)
}

/// Convert eprint's compact archive format (`YYYYMMDD:HHMMSS`) to canonical.
pub fn from_compact(s: &str) -> Result<String, VersionError> {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"^(\d{8}):(\d{6})$").unwrap());
    let caps = re.captures(s).ok_or_else(|| VersionError::Shape { got: s.to_owned() })?;
    Ok(format!("{}T{}Z", &caps[1], &caps[2]))
}

/// Convert OAI-PMH extended format (`YYYY-MM-DDThh:mm:ssZ`) to canonical.
pub fn from_oai(s: &str) -> Result<String, VersionError> {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(
            r"^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})Z$",
        )
        .unwrap()
    });
    let caps = re.captures(s).ok_or_else(|| VersionError::Shape { got: s.to_owned() })?;
    Ok(format!(
        "{}{}{}T{}{}{}Z",
        &caps[1], &caps[2], &caps[3], &caps[4], &caps[5], &caps[6]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_via_compact() {
        assert_eq!(from_compact("20240319:143540").unwrap(), "20240319T143540Z");
    }

    #[test]
    fn round_trips_via_oai() {
        assert_eq!(from_oai("2024-03-19T14:35:40Z").unwrap(), "20240319T143540Z");
    }

    #[test]
    fn rejects_malformed() {
        assert!(!is_canonical("2024-03-19T14:35:40Z"));
        assert!(!is_canonical("20240319143540"));
        assert!(from_compact("bad").is_err());
        assert!(from_oai("bad").is_err());
    }
}
