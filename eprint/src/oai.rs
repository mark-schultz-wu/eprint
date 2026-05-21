//! Minimal OAI-PMH client for `eprint.iacr.org`.
//!
//! We only need `ListRecords` with the `oai_dc` metadata prefix. For each
//! record we extract:
//!
//! - `<identifier>oai:eprint.iacr.org:YYYY/NNNN</identifier>` → `PaperId`
//! - `<datestamp>YYYY-MM-DDThh:mm:ssZ</datestamp>` → modification timestamp
//!
//! Everything else in the metadata block is ignored. We also recognise
//! `<resumptionToken>` and `<error code="...">` for pagination + error
//! handling respectively.

use crate::id::PaperId;
use crate::net::{self, RateLimiter};
use anyhow::{Context as _, Result};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::str::FromStr;
use tracing::{info, info_span, Instrument};

pub const BASE_URL: &str = "https://eprint.iacr.org/oai";

/// Expected OAI-PMH datestamp shape: `YYYY-MM-DDThh:mm:ssZ`, fixed-width UTC.
/// Sort order on this format == chronological order, so callers can use
/// [`datestamp_cmp`] to compare two values.
///
/// If eprint ever changes format (milliseconds, timezone offsets, etc.),
/// callers will see [`DatestampError::Shape`] and we'll know to update.
pub const DATESTAMP_REGEX: &str = r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$";

/// One record's signal from the OAI-PMH response: which paper, and when
/// did its metadata last change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordHeader {
    pub id: PaperId,
    /// ISO 8601 timestamp; matches [`DATESTAMP_REGEX`]. Compare via
    /// [`datestamp_cmp`], not raw `<`/`>`.
    pub datestamp: String,
}

/// Errors comparing OAI datestamps.
#[derive(Debug, thiserror::Error)]
pub enum DatestampError {
    #[error(
        "OAI-PMH datestamp {got:?} doesn't match expected shape \
         YYYY-MM-DDThh:mm:ssZ; eprint may have changed its schema"
    )]
    Shape { got: String },
}

/// Compare two OAI datestamps, returning their chronological ordering.
///
/// Validates both strings against [`DATESTAMP_REGEX`] first; if eprint
/// ever changes the format, callers see [`DatestampError::Shape`]
/// instead of subtly-wrong byte-comparison results.
pub fn datestamp_cmp(a: &str, b: &str) -> Result<std::cmp::Ordering, DatestampError> {
    if !is_valid_datestamp(a) {
        return Err(DatestampError::Shape { got: a.to_owned() });
    }
    if !is_valid_datestamp(b) {
        return Err(DatestampError::Shape { got: b.to_owned() });
    }
    // Once both match the fixed-width regex, lexicographic byte order
    // == chronological order.
    Ok(a.cmp(b))
}

/// Returns true if `s` matches `YYYY-MM-DDThh:mm:ssZ`.
pub fn is_valid_datestamp(s: &str) -> bool {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(DATESTAMP_REGEX).unwrap())
        .is_match(s)
}

/// Outcome of one OAI-PMH page parse.
#[derive(Debug, Default)]
pub struct PageResult {
    pub records: Vec<RecordHeader>,
    pub resumption_token: Option<String>,
    /// `noRecordsMatch` is a legitimate empty result; other codes propagate as errors.
    pub no_records_match: bool,
}

/// Drive `ListRecords` to completion, following resumption tokens.
///
/// `from` is an ISO 8601 date or datetime ("2026-05-21" or
/// "2026-05-21T00:00:00Z"). Pass `None` to omit (= sync from the
/// beginning of time, which is huge — usually a bad idea).
pub async fn list_records(
    client: &reqwest::Client,
    rl: &RateLimiter,
    from: Option<&str>,
) -> Result<Vec<RecordHeader>> {
    let span = info_span!("oai_list_records", from = from.unwrap_or("(beginning)"));
    async {
        let mut out: Vec<RecordHeader> = Vec::new();
        let mut url = first_url(from);
        let mut page_num = 1u32;
        loop {
            let body = net::get_text(client, rl, &url).await?;
            let page = parse_page(&body).context("parsing OAI-PMH response")?;
            info!(
                page = page_num,
                records_on_page = page.records.len(),
                total_so_far = out.len() + page.records.len(),
                "OAI-PMH page fetched"
            );
            if page.no_records_match {
                break;
            }
            out.extend(page.records);
            match page.resumption_token {
                Some(token) if !token.is_empty() => {
                    url = format!(
                        "{BASE_URL}?verb=ListRecords&resumptionToken={}",
                        urlencode(&token)
                    );
                    page_num += 1;
                }
                _ => break,
            }
        }
        Ok(out)
    }
    .instrument(span)
    .await
}

fn first_url(from: Option<&str>) -> String {
    let mut url = format!("{BASE_URL}?verb=ListRecords&metadataPrefix=oai_dc");
    if let Some(f) = from {
        url.push_str("&from=");
        url.push_str(&urlencode(f));
    }
    url
}

/// Very small URL-encoder for the few special chars we emit (`:`, `T`, `Z`,
/// digit-rich tokens). Avoids pulling in `percent-encoding` for this one use.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b':' => {
                out.push(b as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Parse one OAI-PMH response page.
pub fn parse_page(xml: &str) -> Result<PageResult> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut out = PageResult::default();
    let mut buf = Vec::new();
    let mut in_header = false;
    let mut current_field: Option<HeaderField> = None;
    let mut current_id: Option<PaperId> = None;
    let mut current_datestamp: Option<String> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let local = local_name(e.name().as_ref());
                match local.as_str() {
                    "header" => {
                        in_header = true;
                        current_id = None;
                        current_datestamp = None;
                    }
                    "identifier" if in_header => current_field = Some(HeaderField::Identifier),
                    "datestamp" if in_header => current_field = Some(HeaderField::Datestamp),
                    "resumptionToken" => current_field = Some(HeaderField::ResumptionToken),
                    "error" => {
                        let code = attr(&e, "code");
                        if code.as_deref() == Some("noRecordsMatch") {
                            out.no_records_match = true;
                        } else {
                            anyhow::bail!(
                                "OAI-PMH error: code={} message=(see body)",
                                code.unwrap_or_else(|| "unknown".into())
                            );
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(e)) => {
                // `<resumptionToken/>` self-closed = empty (no more pages).
                // `<error code="..."/>` self-closed = error.
                let local = local_name(e.name().as_ref());
                if local == "error" {
                    let code = attr(&e, "code");
                    if code.as_deref() == Some("noRecordsMatch") {
                        out.no_records_match = true;
                    } else {
                        anyhow::bail!(
                            "OAI-PMH error: code={}",
                            code.unwrap_or_else(|| "unknown".into())
                        );
                    }
                }
            }
            Ok(Event::Text(t)) => {
                let text = t.unescape().unwrap_or_default().into_owned();
                match current_field {
                    Some(HeaderField::Identifier) if in_header => {
                        if let Some(id) = parse_oai_identifier(&text) {
                            current_id = Some(id);
                        }
                    }
                    Some(HeaderField::Datestamp) if in_header => {
                        current_datestamp = Some(text);
                    }
                    Some(HeaderField::ResumptionToken) => {
                        out.resumption_token = Some(text);
                    }
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                let local = local_name(e.name().as_ref());
                match local.as_str() {
                    "header" => {
                        in_header = false;
                        if let (Some(id), Some(ds)) = (current_id.take(), current_datestamp.take())
                        {
                            out.records.push(RecordHeader { id, datestamp: ds });
                        }
                    }
                    "identifier" | "datestamp" | "resumptionToken" => {
                        current_field = None;
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(anyhow::anyhow!("XML parse error at position {}: {e}", reader.buffer_position()))
            }
            _ => {}
        }
        buf.clear();
    }

    Ok(out)
}

#[derive(Copy, Clone)]
enum HeaderField {
    Identifier,
    Datestamp,
    ResumptionToken,
}

fn local_name(raw: &[u8]) -> String {
    // Strip XML namespace prefix; we treat all elements unqualified.
    let s = std::str::from_utf8(raw).unwrap_or("");
    match s.rsplit_once(':') {
        Some((_, local)) => local.to_owned(),
        None => s.to_owned(),
    }
}

fn attr(e: &quick_xml::events::BytesStart<'_>, key: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == key.as_bytes())
        .and_then(|a| String::from_utf8(a.value.into_owned()).ok())
}

/// `oai:eprint.iacr.org:YYYY/NNNN` → `PaperId`.
fn parse_oai_identifier(s: &str) -> Option<PaperId> {
    let inner = s.strip_prefix("oai:eprint.iacr.org:")?;
    PaperId::from_str(inner).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<OAI-PMH xmlns="http://www.openarchives.org/OAI/2.0/">
  <responseDate>2026-05-21T20:00:00Z</responseDate>
  <ListRecords>
    <record>
      <header>
        <identifier>oai:eprint.iacr.org:2026/1018</identifier>
        <datestamp>2026-05-21T08:48:16Z</datestamp>
      </header>
      <metadata><foo/></metadata>
    </record>
    <record>
      <header>
        <identifier>oai:eprint.iacr.org:2024/463</identifier>
        <datestamp>2024-03-15T12:00:00Z</datestamp>
      </header>
    </record>
    <resumptionToken>token-for-next-page</resumptionToken>
  </ListRecords>
</OAI-PMH>"##;

    const SAMPLE_NO_RECORDS: &str = r##"<?xml version="1.0"?>
<OAI-PMH>
  <responseDate>2026-05-21T20:00:00Z</responseDate>
  <error code="noRecordsMatch"/>
</OAI-PMH>"##;

    #[test]
    fn parses_two_records_and_token() {
        let p = parse_page(SAMPLE).unwrap();
        assert_eq!(p.records.len(), 2);
        assert_eq!(p.records[0].id.year, 2026);
        assert_eq!(p.records[0].id.num, 1018);
        assert_eq!(p.records[0].datestamp, "2026-05-21T08:48:16Z");
        assert_eq!(p.records[1].id.canonical(), "2024/463");
        assert_eq!(p.resumption_token.as_deref(), Some("token-for-next-page"));
        assert!(!p.no_records_match);
    }

    #[test]
    fn handles_no_records_match_empty_element() {
        let p = parse_page(SAMPLE_NO_RECORDS).unwrap();
        assert!(p.no_records_match);
        assert!(p.records.is_empty());
    }

    #[test]
    fn urlencodes_ts() {
        assert_eq!(urlencode("2026-05-21T08:48:16Z"), "2026-05-21T08:48:16Z");
        assert_eq!(urlencode("a b"), "a%20b");
    }

    #[test]
    fn rejects_non_eprint_identifier() {
        assert!(parse_oai_identifier("oai:arxiv.org:2024.0001").is_none());
        assert_eq!(
            parse_oai_identifier("oai:eprint.iacr.org:2024/463"),
            Some(PaperId { year: 2024, num: 463 })
        );
    }

    #[test]
    fn validates_datestamp_shape() {
        assert!(is_valid_datestamp("2026-05-21T08:48:16Z"));
        assert!(!is_valid_datestamp("2026-05-21")); // date only
        assert!(!is_valid_datestamp("2026-05-21T08:48:16+00:00")); // tz offset
        assert!(!is_valid_datestamp("2026-05-21T08:48:16.123Z")); // ms
        assert!(!is_valid_datestamp(""));
    }

    #[test]
    fn datestamp_cmp_orders_chronologically() {
        let a = "2026-05-21T08:48:16Z";
        let b = "2026-05-21T09:00:00Z";
        let c = "2027-01-01T00:00:00Z";
        assert!(datestamp_cmp(a, b).unwrap().is_lt());
        assert!(datestamp_cmp(b, a).unwrap().is_gt());
        assert!(datestamp_cmp(a, a).unwrap().is_eq());
        assert!(datestamp_cmp(b, c).unwrap().is_lt());
    }

    #[test]
    fn datestamp_cmp_rejects_malformed() {
        let good = "2026-05-21T08:48:16Z";
        assert!(matches!(
            datestamp_cmp("2026-05-21", good),
            Err(DatestampError::Shape { .. })
        ));
        assert!(matches!(
            datestamp_cmp(good, "garbage"),
            Err(DatestampError::Shape { .. })
        ));
    }
}
