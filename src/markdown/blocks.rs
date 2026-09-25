//! Turn MinerU2.5's per-page blocks into Markdown.
//!
//! The model emits typed blocks (`text`, `title`, `equation`, `table`, …) in
//! reading order, with math written as LaTeX `\( … \)` / `\[ … \]`. This module
//! maps them onto Markdown with `$ … $` / `$$ … $$` math and drops page furniture
//! (running headers, page numbers).

use oar_ocr_vl::DocumentBlock;
use regex::Regex;
use std::sync::OnceLock;

/// Page furniture that would pollute the body text.
const DROP: &[&str] = &["header", "footer", "page_number", "page_footnote_mark"];

/// Render one page's blocks. Empty blocks (layout containers such as `list` /
/// `equation_block`, and figures, whose content is pixels) are skipped.
pub fn page_to_markdown(blocks: &[DocumentBlock]) -> Vec<String> {
    blocks.iter().filter_map(block_to_markdown).collect()
}

/// Join rendered pages into a document.
pub fn join(pages: impl IntoIterator<Item = Vec<String>>) -> String {
    let parts: Vec<String> = pages.into_iter().flatten().collect();
    let mut out = parts.join("\n\n");
    out.push('\n');
    out
}

fn block_to_markdown(block: &DocumentBlock) -> Option<String> {
    let content = block.content.as_deref().unwrap_or("").trim();
    if content.is_empty() || DROP.contains(&block.block_type.as_str()) {
        return None;
    }
    Some(match block.block_type.as_str() {
        // MinerU doesn't distinguish heading levels.
        "title" => format!("## {}", inline_math(content)),
        "equation" => format!("$$\n{}\n$$", strip_display_delims(content)),
        "code" => format!("```\n{content}\n```"),
        // Text, lists, captions, footnotes, and HTML tables all carry inline math.
        _ => inline_math(content),
    })
}

fn strip_display_delims(s: &str) -> &str {
    let s = s.trim();
    let s = s.strip_prefix(r"\[").unwrap_or(s);
    s.strip_suffix(r"\]").unwrap_or(s).trim()
}

/// `\( x \)` → `$x$`, `\[ x \]` → `$$x$$`, and undo the model's habit of
/// reading a boxed citation link (`[5]`) as `\( \boxed{5} \)`.
fn inline_math(s: &str) -> String {
    static BOXED: OnceLock<Regex> = OnceLock::new();
    static INLINE: OnceLock<Regex> = OnceLock::new();
    static DISPLAY: OnceLock<Regex> = OnceLock::new();
    let boxed = BOXED
        .get_or_init(|| Regex::new(r"\\\(\s*\\boxed\{([\w\s,.+\-]+)\}\s*\\\)").unwrap());
    let inline = INLINE.get_or_init(|| Regex::new(r"(?s)\\\(\s*(.*?)\s*\\\)").unwrap());
    let display = DISPLAY.get_or_init(|| Regex::new(r"(?s)\\\[\s*(.*?)\s*\\\]").unwrap());
    let s = boxed.replace_all(s, "[$1]");
    let s = inline.replace_all(&s, "$$$1$$");
    display.replace_all(&s, "$$$$$1$$$$").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(kind: &str, content: Option<&str>) -> DocumentBlock {
        DocumentBlock {
            block_type: kind.into(),
            bbox: [0.0; 4],
            angle: None,
            content: content.map(Into::into),
        }
    }

    #[test]
    fn maps_block_types() {
        let page = page_to_markdown(&[
            b("header", Some("Amadou TALL")),
            b("page_number", Some("2")),
            b("title", Some("2 Main results")),
            b("text", Some(r"for \( 2^n - 1 \) we have")),
            b("equation", Some(r"\[\ell (2 ^ {n} - 1) \leq \ell (n)\]")),
            b("code", Some("alert udp any")),
            b("list", Some("")),
            b("image", None),
        ]);
        assert_eq!(
            page,
            vec![
                "## 2 Main results",
                "for $2^n - 1$ we have",
                "$$\n\\ell (2 ^ {n} - 1) \\leq \\ell (n)\n$$",
                "```\nalert udp any\n```",
            ]
        );
    }

    #[test]
    fn boxed_citations_become_brackets() {
        assert_eq!(inline_math(r"Subbarao \( \boxed{5} \) have"), "Subbarao [5] have");
        assert_eq!(inline_math(r"see \( \boxed{9,8} \)"), "see [9,8]");
        // A real boxed formula is left alone.
        assert_eq!(inline_math(r"\( \boxed{x^2} \)"), r"$\boxed{x^2}$");
    }

    #[test]
    fn inline_display_math_inside_text() {
        assert_eq!(inline_math(r"so \[ a = b \] holds"), "so $$a = b$$ holds");
    }

    #[test]
    fn join_separates_blocks_and_pages() {
        let doc = join([vec!["a".to_owned(), "b".to_owned()], vec!["c".to_owned()]]);
        assert_eq!(doc, "a\n\nb\n\nc\n");
    }
}
