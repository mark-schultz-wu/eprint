//! Scrape `eprint.iacr.org/archive/versions/<year>/<num>` for the list of
//! all known versions of a paper. Each entry looks like:
//!
//! ```html
//! <li><a href="/archive/2024/463/20250106:174348">20250106:174348</a> PDF update (most recent)</li>
//! ```
//!
//! `parse_archive_page` is a pure function that returns a typed error
//! when the page looks like a Versions page but no entries match — that
//! signals an upstream template change. Top-level callers can demote to
//! a warn and fall back to whatever they already have.

use crate::net::{self, RateLimiter};
use crate::version;
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub struct ArchiveVersion {
    pub timestamp: String,
    pub is_current: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error(
        "archive page advertises versions but no entries matched the \
         expected `<li><a href=\"/archive/...\">.. PDF update (...)</li>` \
         pattern; eprint may have changed the template"
    )]
    VersionsPageButZeroEntries,
    #[error("malformed archive timestamp: {0}")]
    BadTimestamp(#[from] version::VersionError),
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("http error: {0}")]
    Http(#[from] anyhow::Error),
    #[error(transparent)]
    Parse(#[from] ParseError),
}

/// Fetch and parse the archive page for one paper. Versions returned in
/// ascending chronological order.
pub async fn fetch_versions(
    client: &reqwest::Client,
    rl: &RateLimiter,
    archive_url: &str,
) -> Result<Vec<ArchiveVersion>, Error> {
    let body = net::get_text(client, rl, archive_url).await?;
    parse_archive_page(&body).map_err(Into::into)
}

/// Parse archive HTML. Returns versions ascending by timestamp.
///
/// Errors if the HTML looks like a Versions page (carries the "Versions
/// for ePrint paper" header) but yields zero entries — that's almost
/// certainly a template change worth flagging.
pub fn parse_archive_page(html: &str) -> Result<Vec<ArchiveVersion>, ParseError> {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
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
    if out.is_empty() && looks_like_versions_page(html) {
        return Err(ParseError::VersionsPageButZeroEntries);
    }
    out.reverse(); // page lists newest-first; we want ascending
    Ok(out)
}

fn looks_like_versions_page(html: &str) -> bool {
    html.contains("Versions for ePrint paper") || html.contains("Versions for ePrint")
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
        assert_eq!(v[2].timestamp, "20250106T174348Z");
        assert!(v[2].is_current);
    }

    #[test]
    fn truly_empty_page_returns_empty_vec() {
        let v = parse_archive_page("<html>nothing here</html>").unwrap();
        assert!(v.is_empty());
    }

    #[test]
    fn versions_page_with_no_matches_returns_error() {
        let html = r##"<html><body>
<h2>Versions for ePrint paper 2024/999</h2>
<p>template was rewritten and our regex no longer matches</p>
</body></html>"##;
        let err = parse_archive_page(html).unwrap_err();
        assert!(matches!(err, ParseError::VersionsPageButZeroEntries));
    }
}
