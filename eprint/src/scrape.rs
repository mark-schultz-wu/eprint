//! Minimal HTML scraping of eprint.iacr.org landing pages.
//!
//! Targets:
//! - `<title>...</title>` and `<h3 class="mb-3">...</h3>` → paper title
//! - `<h5 ...>Abstract</h5>` followed by `<p ...>...</p>` → abstract
//! - `<pre id="bibtex">...</pre>` → BibTeX entry
//!
//! We use precise regex patterns over the raw HTML rather than a full
//! parser. eprint's landing page template is stable enough that this is
//! cheaper than pulling in `scraper` / `html5ever`. If their template
//! changes, the tests + an integration test on the live site will catch
//! it.

use std::sync::OnceLock;

#[derive(Debug, Clone, Default)]
pub struct Landing {
    pub title: Option<String>,
    pub abstract_: Option<String>,
    pub bibtex: Option<String>,
}

pub fn parse(html: &str) -> Landing {
    Landing {
        title: extract_title(html),
        abstract_: extract_abstract(html),
        bibtex: extract_bibtex(html),
    }
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
    // BibTeX is in a <pre>; preserve newlines, just decode HTML entities.
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
        regex::RegexBuilder::new(
            r#"<h5[^>]*>\s*Abstract\s*</h5>\s*<p[^>]*>(.+?)</p>"#,
        )
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
    // Minimal: handle the entities we've actually seen in eprint pages.
    // Full decoding would pull in `html-escape`; not worth it.
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
<h5 class="mt-3">Abstract</h5>
<p style="white-space: pre-wrap;">Fully Homomorphic Encryption (FHE) is a cryptographic primitive that allows performing arbitrary operations on encrypted data.</p>
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
        let l = parse(SAMPLE);
        assert_eq!(l.title.as_deref(), Some("Security Guidelines for Implementing Homomorphic Encryption"));
    }

    #[test]
    fn extracts_abstract() {
        let l = parse(SAMPLE);
        assert!(l.abstract_.unwrap().starts_with("Fully Homomorphic Encryption"));
    }

    #[test]
    fn extracts_bibtex() {
        let l = parse(SAMPLE);
        let b = l.bibtex.unwrap();
        assert!(b.starts_with("@misc{cryptoeprint:2024/463"));
        assert!(b.ends_with('}'));
    }

    #[test]
    fn decodes_entities() {
        assert_eq!(decode_entities("a&amp;b"), "a&b");
        assert_eq!(decode_entities("&#39;hi&#39;"), "'hi'");
    }
}
