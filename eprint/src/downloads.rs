//! Locate locally delivered PDFs in the downloads dir.
//!
//! A PDF saved here under the canonical name `<year>-<num>.pdf` (e.g.
//! `2024-1234.pdf`) — by hand or by the companion watcher — is used as the
//! current version's bytes instead of fetching them over HTTP. Metadata
//! (title, BibTeX, abstract, version listing) is still fetched over the
//! network as usual.

use crate::id::PaperId;
use std::path::{Path, PathBuf};

/// Canonical delivered filename for a paper id, matching what the watcher
/// writes. eprint paper numbers carry no leading zeros, so neither does
/// this name (`2024-463.pdf`, not `2024-0463.pdf`).
pub fn pdf_filename(id: PaperId) -> String {
    format!("{}-{}.pdf", id.year, id.num)
}

/// Path where a delivered PDF for `id` would live, whether or not it exists.
pub fn expected_pdf_path(dir: &Path, id: PaperId) -> PathBuf {
    dir.join(pdf_filename(id))
}

/// The delivered PDF for `id`, if a regular file is present.
pub fn local_pdf(dir: &Path, id: PaperId) -> Option<PathBuf> {
    let p = expected_pdf_path(dir, id);
    p.is_file().then_some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_has_no_leading_zeros() {
        assert_eq!(pdf_filename(PaperId { year: 2024, num: 463 }), "2024-463.pdf");
        assert_eq!(pdf_filename(PaperId { year: 2025, num: 7 }), "2025-7.pdf");
    }

    #[test]
    fn local_pdf_finds_and_misses() {
        let dir = tempfile::tempdir().unwrap();
        let id = PaperId { year: 2024, num: 1234 };
        assert!(local_pdf(dir.path(), id).is_none());
        std::fs::write(dir.path().join("2024-1234.pdf"), b"%PDF-1.7\n").unwrap();
        assert_eq!(local_pdf(dir.path(), id).unwrap(), dir.path().join("2024-1234.pdf"));
    }
}
