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

/// Convert canonical `YYYYMMDDTHHMMSSZ` to unix seconds (UTC).
/// Inverse of `days_to_ymd` used by `sync::iso_date_from_unix`.
pub fn to_unix(canonical: &str) -> Result<i64, VersionError> {
    if !is_canonical(canonical) {
        return Err(VersionError::Shape { got: canonical.to_owned() });
    }
    let b = canonical.as_bytes();
    let y: i64 = parse_n(&b[0..4]);
    let m: i64 = parse_n(&b[4..6]);
    let d: i64 = parse_n(&b[6..8]);
    let hh: i64 = parse_n(&b[9..11]);
    let mm: i64 = parse_n(&b[11..13]);
    let ss: i64 = parse_n(&b[13..15]);
    let days = days_since_epoch(y, m, d);
    Ok(days * 86_400 + hh * 3600 + mm * 60 + ss)
}

fn parse_n(b: &[u8]) -> i64 {
    std::str::from_utf8(b).unwrap().parse().unwrap()
}

/// Days from 1970-01-01 to (y, m, d). Inverse of the `ymd_from_days_since_epoch`
/// formula used in `commands::sync`. Howard Hinnant's algorithm.
fn days_since_epoch(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m_adj = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * m_adj + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

#[cfg(test)]
mod unix_tests {
    use super::*;

    #[test]
    fn to_unix_known_points() {
        // Verified via `date -u --date='2024-03-19 14:35:40' +%s` = 1710858940
        assert_eq!(to_unix("20240319T143540Z").unwrap(), 1710858940);
        assert_eq!(to_unix("19700101T000000Z").unwrap(), 0);
        // 2025-01-06 17:43:48 UTC = 1736185428
        assert_eq!(to_unix("20250106T174348Z").unwrap(), 1736185428);
    }
}
