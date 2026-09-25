//! Minimal HTML scraping of eprint.iacr.org landing pages.
//!
//! Targets:
//! - `<h3 class="mb-3">...</h3>` → paper title
//! - `<h5 ...>Abstract</h5>` followed by `<p ...>...</p>` → abstract
//! - `<pre id="bibtex">...</pre>` → BibTeX entry
//!
//! We use precise regex patterns over the raw HTML rather than a full
//! parser. eprint's landing page template is stable enough that this is
//! cheaper than pulling in `scraper` / `html5ever`.
//!
//! `parse` returns a typed error when the page clearly *looks* like a
//! paper landing page (has the structural markers) but no title was
//! extracted — that's the canonical signal that the template has drifted
//! and our regexes need updating. Callers can demote to a warn so that
//! the rest of the fetch (PDF, version meta) still succeeds.

use std::sync::OnceLock;

#[derive(Debug, Clone, Default)]
pub struct Landing {
    pub title: Option<String>,
    pub abstract_: Option<String>,
    pub bibtex: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error(
        "landing page looks like an ePrint paper page but no title matched the \
         expected `<h3 class=\"mb-3\">...</h3>` pattern; eprint may have changed \
         the template"
    )]
    NoTitleOnPaperPage,
}

/// Parse an eprint landing page.
///
/// Errors iff the page contains the expected paper-landing structure but
/// the title couldn't be extracted. Individual missing fields (e.g. a
/// paper genuinely has no abstract on file) come back as `None` without
/// erroring.
pub fn parse(html: &str) -> Result<Landing, ParseError> {
    let title = extract_title(html);
    if title.is_none() && looks_like_paper_landing(html) {
        return Err(ParseError::NoTitleOnPaperPage);
    }
    Ok(Landing {
        title,
        abstract_: extract_abstract(html),
        bibtex: extract_bibtex(html),
    })
}

/// Cheap heuristic: every real paper landing page has at least one of
/// these markers in its `<dt>...</dt>` metadata block. If none match,
/// we got something else entirely (e.g. a 404 stub) and silently
/// returning no fields is the right call.
fn looks_like_paper_landing(html: &str) -> bool {
    html.contains("class=\"author\"")
        || html.contains("<dt>Category</dt>")
        || html.contains("<dt>Publication info</dt>")
        || html.contains("<dt>History</dt>")
}

fn extract_title(html: &str) -> Option<String> {
    let h3 = h3_re().captures(html)?;
    Some(clean_inline(h3.get(1)?.as_str()))
}

fn extract_abstract(html: &str) -> Option<String> {
    let m = abstract_re().captures(html)?;
    Some(clean_inline(m.get(1)?.as_str()))
}

fn extract_bibtex(html: &str) -> Option<String> {
    let m = bibtex_re().captures(html)?;
    Some(decode_entities(m.get(1)?.as_str()).trim().to_owned())
}

fn h3_re() -> &'static regex::Regex {
    static R: OnceLock<regex::Regex> = OnceLock::new();
    R.get_or_init(|| {
        regex::RegexBuilder::new(r#"<h3[^>]*class="mb-3"[^>]*>(.+?)</h3>"#)
            .dot_matches_new_line(true)
            .build()
            .unwrap()
    })
}

fn abstract_re() -> &'static regex::Regex {
    static R: OnceLock<regex::Regex> = OnceLock::new();
    R.get_or_init(|| {
        regex::RegexBuilder::new(r#"<h5[^>]*>\s*Abstract\s*</h5>\s*<p[^>]*>(.+?)</p>"#)
            .dot_matches_new_line(true)
            .case_insensitive(true)
            .build()
            .unwrap()
    })
}

fn bibtex_re() -> &'static regex::Regex {
    static R: OnceLock<regex::Regex> = OnceLock::new();
    R.get_or_init(|| {
        regex::RegexBuilder::new(r#"<pre[^>]*id="bibtex"[^>]*>(.+?)</pre>"#)
            .dot_matches_new_line(true)
            .build()
            .unwrap()
    })
}

/// Strip HTML tags, collapse whitespace, decode entities. For short fields.
fn clean_inline(s: &str) -> String {
    let no_tags = strip_tags_re().replace_all(s, " ");
    let decoded = decode_entities(&no_tags);
    whitespace_re().replace_all(decoded.trim(), " ").into_owned()
}

fn strip_tags_re() -> &'static regex::Regex {
    static R: OnceLock<regex::Regex> = OnceLock::new();
    R.get_or_init(|| regex::Regex::new(r"<[^>]+>").unwrap())
}

fn whitespace_re() -> &'static regex::Regex {
    static R: OnceLock<regex::Regex> = OnceLock::new();
    R.get_or_init(|| regex::Regex::new(r"\s+").unwrap())
}

fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r##"
<html><body>
<h3 class="mb-3">Security Guidelines for Implementing Homomorphic Encryption</h3>
<div class="author"><span class="authorName">Anyone</span></div>
<h5 class="mt-3">Abstract</h5>
<p style="white-space: pre-wrap;">Fully Homomorphic Encryption (FHE) is a cryptographic primitive that allows performing arbitrary operations on encrypted data.</p>
<dt>Category</dt><dd>FHE</dd>
<pre id="bibtex">
@misc{cryptoeprint:2024/463,
      title = {Security Guidelines for Implementing Homomorphic Encryption},
      year = {2024}
}
</pre>
</body></html>
"##;

    #[test]
    fn extracts_title() {
        let l = parse(SAMPLE).unwrap();
        assert_eq!(
            l.title.as_deref(),
            Some("Security Guidelines for Implementing Homomorphic Encryption")
        );
    }

    #[test]
    fn extracts_abstract() {
        let l = parse(SAMPLE).unwrap();
        assert!(l.abstract_.unwrap().starts_with("Fully Homomorphic Encryption"));
    }

    #[test]
    fn extracts_bibtex() {
        let l = parse(SAMPLE).unwrap();
        let b = l.bibtex.unwrap();
        assert!(b.starts_with("@misc{cryptoeprint:2024/463"));
        assert!(b.ends_with('}'));
    }

    #[test]
    fn decodes_entities() {
        assert_eq!(decode_entities("a&amp;b"), "a&b");
        assert_eq!(decode_entities("&#39;hi&#39;"), "'hi'");
    }

    /// If eprint's landing template changes such that the `<h3 class="mb-3">`
    /// pattern no longer matches but the page is clearly still a paper
    /// landing, we want a loud signal — not silent `None` everywhere.
    #[test]
    fn errors_when_paper_landing_has_no_extractable_title() {
        let html = r##"
<html><body>
<h2>oh no a different heading</h2>
<div class="author"><span class="authorName">Anyone</span></div>
<dt>Category</dt><dd>FHE</dd>
</body></html>
"##;
        let err = parse(html).unwrap_err();
        assert!(matches!(err, ParseError::NoTitleOnPaperPage));
    }

    /// A truly unrelated page (no landing-page markers) returns an empty
    /// landing, not an error — we don't want to error on every random
    /// HTML response.
    #[test]
    fn unrelated_html_returns_empty_landing() {
        let l = parse("<html>just a 404</html>").unwrap();
        assert!(l.title.is_none() && l.abstract_.is_none() && l.bibtex.is_none());
    }
}
