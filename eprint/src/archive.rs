//! Scrape `eprint.iacr.org/archive/versions/<year>/<num>` for the list of
//! all known versions of a paper. Each entry looks like:
//!
//! ```html
//! <li><a href="/archive/2024/463/20250106:174348">20250106:174348</a> PDF update (most recent)</li>
//! ```
//!
//! We extract the compact timestamp + the "most recent" marker, convert
//! to canonical form, and return.

use crate::net::{self, RateLimiter};
use crate::version;
use anyhow::Result;
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub struct ArchiveVersion {
    /// Canonical timestamp (e.g. `20240319T143540Z`).
    pub timestamp: String,
    /// True if marked "(most recent)" in the listing.
    pub is_current: bool,
}

/// Fetch and parse the archive page for `<id>`. Returns versions in
/// ascending chronological order (oldest first).
pub async fn fetch_versions(
    client: &reqwest::Client,
    rl: &RateLimiter,
    archive_url: &str,
) -> Result<Vec<ArchiveVersion>> {
    let body = net::get_text(client, rl, archive_url).await?;
    parse_archive_page(&body)
}

/// Parse archive HTML. Returns versions ascending by timestamp.
pub fn parse_archive_page(html: &str) -> Result<Vec<ArchiveVersion>> {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        // Match: <li><a href="/archive/YEAR/NUM/TIMESTAMP">TIMESTAMP</a> PDF update (TAIL)</li>
        regex::Regex::new(
            r#"<li>\s*<a[^>]*href="/archive/\d+/\d+/(\d{8}:\d{6})"[^>]*>\s*\d{8}:\d{6}\s*</a>\s*PDF update(?:\s*\(([^)]+)\))?"#,
        )
        .unwrap()
    });
    let mut out = Vec::new();
    for caps in re.captures_iter(html) {
        let compact = caps.get(1).unwrap().as_str();
        let canonical = version::from_compact(compact)?;
        let marker = caps.get(2).map(|m| m.as_str()).unwrap_or("");
        out.push(ArchiveVersion {
            timestamp: canonical,
            is_current: marker.contains("most recent"),
        });
    }
    // Page lists newest-first; reverse to ascending.
    out.reverse();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r##"<html><body>
<h2>Versions for ePrint paper 2024/463</h2>
<ul>
    <li><a href="/archive/2024/463/20250106:174348">20250106:174348</a> PDF update (most recent)</li>
    <li><a href="/archive/2024/463/20241017:150428">20241017:150428</a> PDF update</li>
    <li><a href="/archive/2024/463/20240319:143540">20240319:143540</a> PDF update</li>
</ul>
</body></html>"##;

    #[test]
    fn parses_three_versions_ascending() {
        let v = parse_archive_page(SAMPLE).unwrap();
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].timestamp, "20240319T143540Z");
        assert_eq!(v[1].timestamp, "20241017T150428Z");
        assert_eq!(v[2].timestamp, "20250106T174348Z");
        assert!(v[2].is_current);
        assert!(!v[0].is_current);
    }

    #[test]
    fn handles_empty_archive() {
        let html = "<html><body>no versions here</body></html>";
        let v = parse_archive_page(html).unwrap();
        assert!(v.is_empty());
    }
}
