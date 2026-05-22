//! Typed version-timestamp wrappers backed by `time::OffsetDateTime`.
//!
//! eprint exposes a paper's revision history with timestamps in two
//! string formats, and we use a third for our own storage. Each wrapper
//! validates its expected wire format at construction and then defers
//! all date arithmetic to the `time` crate.
//!
//! ```text
//! ArchiveCompact ─┐
//!                 ├─> Canonical  (storage / cache dir names)
//! OaiDatestamp  ──┘
//! ```

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;
use time::format_description::well_known::Rfc3339;
use time::format_description::FormatItem;
use time::macros::format_description;
use time::{OffsetDateTime, PrimitiveDateTime};

/// Lower bound on the year we accept anywhere. The eprint archive
/// itself started in 1996; this is conservative.
const MIN_YEAR: i32 = 1970;

/// `YYYYMMDDTHHMMSSZ` — ISO 8601 basic UTC. The on-disk + cache-dir form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Canonical(OffsetDateTime);

/// `YYYY-MM-DDThh:mm:ssZ` — OAI-PMH `<datestamp>` form (RFC 3339).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OaiDatestamp(OffsetDateTime);

/// `YYYYMMDD:HHMMSS` — eprint archive-page listing form. Timezone is
/// implicit UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveCompact(OffsetDateTime);

#[derive(Debug, thiserror::Error)]
pub enum VersionError {
    #[error("malformed canonical version {got:?}; expected YYYYMMDDTHHMMSSZ")]
    Canonical { got: String },
    #[error("malformed OAI datestamp {got:?}; expected YYYY-MM-DDThh:mm:ssZ")]
    Oai { got: String },
    #[error("malformed archive timestamp {got:?}; expected YYYYMMDD:HHMMSS")]
    Compact { got: String },
    #[error("year {got} predates 1970 — eprint papers never do")]
    PreEpoch { got: i32 },
}

const CANONICAL_FMT: &[FormatItem<'_>] =
    format_description!("[year][month][day]T[hour][minute][second]Z");

const COMPACT_FMT: &[FormatItem<'_>] =
    format_description!("[year][month][day]:[hour][minute][second]");

// ============================================================
// Canonical
// ============================================================

impl Canonical {
    /// Unix seconds (UTC). Total: the wrapper's invariant (year ≥ 1970,
    /// enforced in every constructor) guarantees the underlying
    /// `OffsetDateTime`'s `unix_timestamp()` is non-negative.
    pub fn to_unix(&self) -> u64 {
        // `unix_timestamp` returns i64; for year ≥ 1970 it's ≥ 0.
        u64::try_from(self.0.unix_timestamp()).expect("year >= 1970 invariant")
    }
}

impl FromStr for Canonical {
    type Err = VersionError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let dt = PrimitiveDateTime::parse(s, CANONICAL_FMT)
            .map_err(|_| VersionError::Canonical { got: s.to_owned() })?
            .assume_utc();
        check_year(dt)?;
        Ok(Canonical(dt))
    }
}

impl fmt::Display for Canonical {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `format` only fails on incompatible format descriptors or
        // out-of-range components; our descriptor matches the wrapper's
        // invariant, so the call cannot fail at runtime.
        f.write_str(&self.0.format(CANONICAL_FMT).expect("format infallible for canonical"))
    }
}

impl Serialize for Canonical {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
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
        let dt = OffsetDateTime::parse(s, &Rfc3339)
            .map_err(|_| VersionError::Oai { got: s.to_owned() })?;
        check_year(dt)?;
        Ok(OaiDatestamp(dt))
    }
}

impl fmt::Display for OaiDatestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(
            &self
                .0
                .format(&Rfc3339)
                .expect("format infallible for RFC3339"),
        )
    }
}

impl From<&OaiDatestamp> for Canonical {
    fn from(src: &OaiDatestamp) -> Canonical {
        // Same instant in time; only the wire format differs.
        Canonical(src.0)
    }
}

// ============================================================
// ArchiveCompact
// ============================================================

impl FromStr for ArchiveCompact {
    type Err = VersionError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let dt = PrimitiveDateTime::parse(s, COMPACT_FMT)
            .map_err(|_| VersionError::Compact { got: s.to_owned() })?
            .assume_utc();
        check_year(dt)?;
        Ok(ArchiveCompact(dt))
    }
}

impl fmt::Display for ArchiveCompact {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.format(COMPACT_FMT).expect("format infallible for compact"))
    }
}

impl From<&ArchiveCompact> for Canonical {
    fn from(src: &ArchiveCompact) -> Canonical {
        Canonical(src.0)
    }
}

// ============================================================
// shared
// ============================================================

fn check_year(dt: OffsetDateTime) -> Result<(), VersionError> {
    if dt.year() < MIN_YEAR {
        return Err(VersionError::PreEpoch { got: dt.year() });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_round_trip() {
        let c: Canonical = "20240319T143540Z".parse().unwrap();
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
        assert_eq!(c.to_string(), "20240319T143540Z");
    }

    #[test]
    fn from_compact_to_canonical() {
        let a: ArchiveCompact = "20240319:143540".parse().unwrap();
        let c: Canonical = (&a).into();
        assert_eq!(c.to_string(), "20240319T143540Z");
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
        assert!(serde_json::from_str::<Canonical>("\"not-a-timestamp\"").is_err());
    }

    #[test]
    fn rejects_pre_1970_years() {
        assert!("19691231T235959Z".parse::<Canonical>().is_err());
        assert!("1969-12-31T23:59:59Z".parse::<OaiDatestamp>().is_err());
        assert!("19691231:235959".parse::<ArchiveCompact>().is_err());
    }
}
