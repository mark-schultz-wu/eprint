//! RSS feed parsing for eprint.iacr.org.
//!
//! The site exposes `/rss/rss.xml` and `/rss/atom.xml`. We use the RSS
//! variant and accept the same `order=recent` + `category=...` query
//! parameters the eprint UI uses:
//!
//! - default (no `order`): items ordered by **last-modified** — new +
//!   revised, mixed in publication-order independent way.
//! - `order=recent`: items ordered by **publication date** — purely new
//!   papers, revisions don't bump.
//!
//! We pull `<title>`, `<link>`, `<pubDate>`, `<dc:creator>` (multiple),
//! `<category>`, and `<description>` per `<item>`. The author tag is
//! `dc:creator`; we accumulate multiple per item.

use quick_xml::events::Event;
use quick_xml::Reader;

pub const RSS_URL: &str = "https://eprint.iacr.org/rss/rss.xml";

#[derive(Debug, Clone, Default)]
pub struct Item {
    pub title: String,
    pub link: String,
    pub pub_date: Option<String>,
    pub category: Option<String>,
    pub authors: Vec<String>,
    pub description: String,
}

pub fn parse_rss(xml: &str) -> Result<Vec<Item>, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut items: Vec<Item> = Vec::new();
    let mut buf = Vec::new();
    let mut current: Option<Item> = None;
    let mut field: Option<Field> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let local = local_name(e.name().as_ref());
                match local.as_str() {
                    "item" => current = Some(Item::default()),
                    "title" => field = Some(Field::Title),
                    "link" => field = Some(Field::Link),
                    "pubDate" => field = Some(Field::PubDate),
                    "category" => field = Some(Field::Category),
                    "creator" => field = Some(Field::Creator), // dc:creator after local-name strip
                    "description" => field = Some(Field::Description),
                    _ => {}
                }
            }
            Ok(Event::Text(t)) => {
                let text = t.unescape().unwrap_or_default().into_owned();
                if let (Some(it), Some(f)) = (current.as_mut(), field) {
                    match f {
                        Field::Title => it.title.push_str(&text),
                        Field::Link => it.link.push_str(&text),
                        Field::PubDate => it.pub_date = Some(text),
                        Field::Category => it.category = Some(text),
                        Field::Creator => it.authors.push(text),
                        Field::Description => it.description.push_str(&text),
                    }
                }
            }
            Ok(Event::CData(c)) => {
                // RSS descriptions sometimes come as CDATA.
                let text = String::from_utf8_lossy(c.as_ref()).into_owned();
                if let (Some(it), Some(Field::Description)) = (current.as_mut(), field) {
                    it.description.push_str(&text);
                }
            }
            Ok(Event::End(e)) => {
                let local = local_name(e.name().as_ref());
                match local.as_str() {
                    "item" => {
                        if let Some(it) = current.take() {
                            items.push(it);
                        }
                    }
                    "title" | "link" | "pubDate" | "category" | "creator" | "description" => {
                        field = None
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("xml parse error: {e}")),
            _ => {}
        }
        buf.clear();
    }
    Ok(items)
}

#[derive(Copy, Clone)]
enum Field {
    Title,
    Link,
    PubDate,
    Category,
    Creator,
    Description,
}

fn local_name(raw: &[u8]) -> String {
    let s = std::str::from_utf8(raw).unwrap_or("");
    match s.rsplit_once(':') {
        Some((_, local)) => local.to_owned(),
        None => s.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r##"<?xml version="1.0"?>
<rss xmlns:dc="http://purl.org/dc/elements/1.1/" version="2.0">
  <channel>
    <item>
      <title>Some Paper Title</title>
      <link>https://eprint.iacr.org/2026/100</link>
      <pubDate>Mon, 19 May 2026 12:00:00 +0000</pubDate>
      <category>Cryptographic protocols</category>
      <dc:creator>Alice</dc:creator>
      <dc:creator>Bob</dc:creator>
      <description>An interesting abstract.</description>
    </item>
    <item>
      <title>Another</title>
      <link>https://eprint.iacr.org/2026/101</link>
      <pubDate>Mon, 19 May 2026 13:00:00 +0000</pubDate>
      <dc:creator>Carol</dc:creator>
    </item>
  </channel>
</rss>"##;

    #[test]
    fn parses_items_and_multiple_authors() {
        let items = parse_rss(SAMPLE).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "Some Paper Title");
        assert_eq!(items[0].link, "https://eprint.iacr.org/2026/100");
        assert_eq!(items[0].authors, vec!["Alice", "Bob"]);
        assert_eq!(items[0].category.as_deref(), Some("Cryptographic protocols"));
        assert_eq!(items[1].authors, vec!["Carol"]);
    }
}
