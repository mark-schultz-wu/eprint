//! Identifiers: which paper ([`PaperId`]) and which revision of it
//! ([`version::Canonical`] and the timestamp formats eprint uses on the wire).

mod paper_id;
pub mod version;

pub use paper_id::PaperId;
