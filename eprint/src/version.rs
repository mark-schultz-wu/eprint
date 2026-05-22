//! Typed version-timestamp wrappers.
//!
//! eprint exposes a paper's revision history with timestamps in two
//! different notations, and we use a third (filesystem-friendly) form
//! for our own storage. Wrapping each in its own newtype lets the
//! type system enforce "you can't pass an OAI string where canonical
//! was expected."
//!
//! Conversions all funnel through [`Canonical`]:
//!
//! ```text
//! ArchiveCompact ─┐
//!                 ├─> Canonical  (storage)
//! OaiDatestamp  ──┘
//! ```

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::str::FromStr;
use std::sync::OnceLock;

/// `YYYYMMDDTHHMMSSZ` — filesystem-friendly basic ISO 8601 UTC. Used
/// for cache directory names and serialised in `PaperMeta`.
///
/// Lexicographic byte ordering on the fixed-width form equals
/// chronological ordering, so we derive `Ord`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Canonical(String);

/// `YYYY-MM-DDThh:mm:ssZ` — OAI-PMH `<datestamp>` form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OaiDatestamp(String);

/// `YYYYMMDD:HHMMSS` — eprint archive-page listing form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveCompact(String);

#[derive(Debug, thiserror::Error)]
pub enum VersionError {
    #[error("malformed canonical version {got:?}; expected YYYYMMDDTHHMMSSZ")]
    Canonical { got: String },
    #[error("malformed OAI datestamp {got:?}; expected YYYY-MM-DDThh:mm:ssZ")]
    Oai { got: String },
    #[error("malformed archive timestamp {got:?}; expected YYYYMMDD:HHMMSS")]
    Compact { got: String },
}

// ============================================================
// Canonical
// ============================================================

impl Canonical {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Unix seconds (UTC). Safe: the inner string passed validation at
    /// construction, so date math can't fail.
    pub fn to_unix(&self) -> i64 {
        let b = self.0.as_bytes();
        let y: i64 = parse_n(&b[0..4]);
        let m: i64 = parse_n(&b[4..6]);
        let d: i64 = parse_n(&b[6..8]);
        let hh: i64 = parse_n(&b[9..11]);
        let mm: i64 = parse_n(&b[11..13]);
        let ss: i64 = parse_n(&b[13..15]);
        days_since_epoch(y, m, d) * 86_400 + hh * 3600 + mm * 60 + ss
    }
}

impl FromStr for Canonical {
    type Err = VersionError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if canonical_re().is_match(s) {
            Ok(Canonical(s.to_owned()))
        } else {
            Err(VersionError::Canonical { got: s.to_owned() })
        }
    }
}

impl std::fmt::Display for Canonical {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for Canonical {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

// Serde: serialize as bare string; deserialize validates via FromStr.
impl Serialize for Canonical {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Canonical {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Canonical::from_str(&s).map_err(serde::de::Error::custom)
    }
}

// ============================================================
// OaiDatestamp
// ============================================================


impl FromStr for OaiDatestamp {
    type Err = VersionError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if oai_re().is_match(s) {
            Ok(OaiDatestamp(s.to_owned()))
        } else {
            Err(VersionError::Oai { got: s.to_owned() })
        }
    }
}

impl std::fmt::Display for OaiDatestamp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&OaiDatestamp> for Canonical {
    fn from(src: &OaiDatestamp) -> Canonical {
        // src is already validated; the conversion just strips
        // separators. Re-validating is cheap insurance.
        let stripped: String = src
            .0
            .chars()
            .filter(|c| c.is_ascii_digit() || *c == 'T' || *c == 'Z')
            .collect();
        Canonical(stripped)
    }
}

// ============================================================
// ArchiveCompact
// ============================================================

impl FromStr for ArchiveCompact {
    type Err = VersionError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if compact_re().is_match(s) {
            Ok(ArchiveCompact(s.to_owned()))
        } else {
            Err(VersionError::Compact { got: s.to_owned() })
        }
    }
}

impl std::fmt::Display for ArchiveCompact {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&ArchiveCompact> for Canonical {
    fn from(src: &ArchiveCompact) -> Canonical {
        // `YYYYMMDD:HHMMSS` → `YYYYMMDDTHHMMSSZ`. Both halves validated.
        let (date, time) = src.0.split_once(':').expect("ArchiveCompact invariant: contains a colon");
        Canonical(format!("{date}T{time}Z"))
    }
}

// ============================================================
// regex helpers (OnceLock, compiled on first use)
// ============================================================

fn canonical_re() -> &'static regex::Regex {
    static R: OnceLock<regex::Regex> = OnceLock::new();
    R.get_or_init(|| regex::Regex::new(r"^\d{8}T\d{6}Z$").unwrap())
}

fn oai_re() -> &'static regex::Regex {
    static R: OnceLock<regex::Regex> = OnceLock::new();
    R.get_or_init(|| {
        regex::Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$").unwrap()
    })
}

fn compact_re() -> &'static regex::Regex {
    static R: OnceLock<regex::Regex> = OnceLock::new();
    R.get_or_init(|| regex::Regex::new(r"^\d{8}:\d{6}$").unwrap())
}

// ============================================================
// Date math (only callable from a validated Canonical)
// ============================================================

fn parse_n(b: &[u8]) -> i64 {
    // Safety: caller guarantees `b` contains ASCII digits (regex-checked
    // at construction). Reused across all four call sites in to_unix.
    std::str::from_utf8(b).unwrap().parse().unwrap()
}

/// Days from 1970-01-01 to (y, m, d). Howard Hinnant's algorithm.
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
mod tests {
    use super::*;

    #[test]
    fn canonical_round_trip() {
        let c: Canonical = "20240319T143540Z".parse().unwrap();
        assert_eq!(c.as_str(), "20240319T143540Z");
        assert_eq!(c.to_string(), "20240319T143540Z");
    }

    #[test]
    fn canonical_rejects_oai_shape() {
        // The whole POINT of typed wrappers: an OAI-shaped string is
        // NOT a Canonical.
        assert!("2024-03-19T14:35:40Z".parse::<Canonical>().is_err());
        assert!("20240319143540".parse::<Canonical>().is_err());
    }

    #[test]
    fn from_oai_to_canonical() {
        let o: OaiDatestamp = "2024-03-19T14:35:40Z".parse().unwrap();
        let c: Canonical = (&o).into();
        assert_eq!(c.as_str(), "20240319T143540Z");
    }

    #[test]
    fn from_compact_to_canonical() {
        let a: ArchiveCompact = "20240319:143540".parse().unwrap();
        let c: Canonical = (&a).into();
        assert_eq!(c.as_str(), "20240319T143540Z");
    }

    #[test]
    fn canonical_ord_is_chronological() {
        let early: Canonical = "20240319T143540Z".parse().unwrap();
        let late: Canonical = "20250106T174348Z".parse().unwrap();
        assert!(early < late);
        assert!(late > early);
    }

    #[test]
    fn to_unix_known_points() {
        let c: Canonical = "20240319T143540Z".parse().unwrap();
        assert_eq!(c.to_unix(), 1710858940);
        let epoch: Canonical = "19700101T000000Z".parse().unwrap();
        assert_eq!(epoch.to_unix(), 0);
        let c2: Canonical = "20250106T174348Z".parse().unwrap();
        assert_eq!(c2.to_unix(), 1736185428);
    }

    #[test]
    fn serde_round_trips_as_bare_string() {
        let c: Canonical = "20240319T143540Z".parse().unwrap();
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(json, "\"20240319T143540Z\"");
        let back: Canonical = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn serde_rejects_invalid_on_deserialize() {
        // A bogus string fails to deserialize — keeps invariants intact.
        assert!(serde_json::from_str::<Canonical>("\"not-a-timestamp\"").is_err());
    }
}
