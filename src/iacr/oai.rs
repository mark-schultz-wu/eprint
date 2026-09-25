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

use crate::iacr::http::{self, RateLimiter};
use crate::ids::PaperId;
use anyhow::{Context as _, Result};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::str::FromStr;
use tracing::{info, info_span, Instrument};

/// One record's signal from the OAI-PMH response: which paper, and when
/// did its metadata last change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordHeader {
    pub id: PaperId,
    /// ISO 8601 extended timestamp `YYYY-MM-DDThh:mm:ssZ`. Convert to
    /// canonical via `crate::ids::version::from_oai` before comparing/storing.
    pub datestamp: String,
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
    endpoint: &str,
    from: Option<&str>,
) -> Result<Vec<RecordHeader>> {
    let span = info_span!("oai_list_records", from = from.unwrap_or("(beginning)"));
    async {
        let mut out: Vec<RecordHeader> = Vec::new();
        let mut url = first_url(endpoint, from);
        let mut page_num = 1u32;
        loop {
            let body = http::get_text(client, rl, &url).await?;
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
                // The last page's token is an empty element, which the reader
                // reports as no text: `None`.
                Some(token) => {
                    url = format!(
                        "{endpoint}?verb=ListRecords&resumptionToken={}",
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

fn first_url(endpoint: &str, from: Option<&str>) -> String {
    let mut url = format!("{endpoint}?verb=ListRecords&metadataPrefix=oai_dc");
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

/// One paper's current metadata from an OAI-PMH `GetRecord` (oai_dc).
///
/// Fallback for version discovery + title/abstract when the archive listing
/// can't be scraped (network error, rate limit, template drift), so a paper
/// still gets a current version to file its PDF under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// `YYYY-MM-DDThh:mm:ssZ` last-modified stamp = the current version.
    pub datestamp: String,
    pub title: Option<String>,
    pub abstract_: Option<String>,
}

/// Fetch one paper's record via `GetRecord` + `oai_dc`. `Ok(None)` when the
/// paper has no OAI record (`idDoesNotExist`).
pub async fn get_record(
    client: &reqwest::Client,
    rl: &RateLimiter,
    endpoint: &str,
    id: PaperId,
) -> Result<Option<Record>> {
    let url = format!(
        "{endpoint}?verb=GetRecord&identifier={}&metadataPrefix=oai_dc",
        id.oai_identifier()
    );
    let body = http::get_text(client, rl, &url).await?;
    parse_record(&body).context("parsing OAI-PMH GetRecord response")
}

/// Parse a `GetRecord` response: the header `datestamp` plus `dc:title` and
/// `dc:description` from the `oai_dc` metadata. `Ok(None)` for a missing
/// record; `Err` for other OAI errors.
pub fn parse_record(xml: &str) -> Result<Option<Record>> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut field: Option<RecField> = None;
    let mut datestamp: Option<String> = None;
    let mut title: Option<String> = None;
    let mut abstract_: Option<String> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => match local_name(e.name().as_ref()).as_str() {
                "datestamp" => field = Some(RecField::Datestamp),
                "title" => field = Some(RecField::Title),
                "description" => field = Some(RecField::Description),
                "error" => return on_record_error(&e),
                _ => {}
            },
            Ok(Event::Empty(e)) => {
                if local_name(e.name().as_ref()) == "error" {
                    return on_record_error(&e);
                }
            }
            Ok(Event::Text(t)) => {
                let text = t.unescape().unwrap_or_default().into_owned();
                match field {
                    Some(RecField::Datestamp) if datestamp.is_none() => datestamp = Some(text),
                    Some(RecField::Title) if title.is_none() => title = Some(text),
                    Some(RecField::Description) if abstract_.is_none() => abstract_ = Some(text),
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                if matches!(
                    local_name(e.name().as_ref()).as_str(),
                    "datestamp" | "title" | "description"
                ) {
                    field = None;
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "XML parse error at position {}: {e}",
                    reader.buffer_position()
                ))
            }
            _ => {}
        }
        buf.clear();
    }

    Ok(datestamp.map(|ds| Record {
        datestamp: ds,
        title: title.filter(|s| !s.is_empty()),
        abstract_: abstract_.filter(|s| !s.is_empty()),
    }))
}

/// A missing record is `Ok(None)`; any other OAI error code propagates.
fn on_record_error(e: &quick_xml::events::BytesStart<'_>) -> Result<Option<Record>> {
    match attr(e, "code").as_deref() {
        Some("idDoesNotExist") | Some("noRecordsMatch") => Ok(None),
        other => anyhow::bail!(
            "OAI-PMH GetRecord error: code={}",
            other.unwrap_or("unknown")
        ),
    }
}

#[derive(Copy, Clone)]
enum RecField {
    Datestamp,
    Title,
    Description,
}

/// Parse one OAI-PMH response page.
pub fn parse_page(xml: &str) -> Result<PageResult> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut out = PageResult::default();
    let mut buf = Vec::new();
    let mut current_field: Option<HeaderField> = None;
    let mut current_id: Option<PaperId> = None;
    let mut current_datestamp: Option<String> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let local = local_name(e.name().as_ref());
                match local.as_str() {
                    "header" => {
                        current_id = None;
                        current_datestamp = None;
                    }
                    // Records also carry <identifier>/<datestamp> outside the
                    // header (dc:identifier, provenance in <about>). Those come
                    // after </header>, where the record has already been
                    // emitted, and are cleared at the next <header>, so only
                    // the header's values ever describe a record.
                    "identifier" => current_field = Some(HeaderField::Identifier),
                    "datestamp" => current_field = Some(HeaderField::Datestamp),
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
                    Some(HeaderField::Identifier) => {
                        if let Some(id) = parse_oai_identifier(&text) {
                            current_id = Some(id);
                        }
                    }
                    Some(HeaderField::Datestamp) => {
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
                return Err(anyhow::anyhow!(
                    "XML parse error at position {}: {e}",
                    reader.buffer_position()
                ))
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

    const GETRECORD_SAMPLE: &str = r##"<?xml version="1.0"?>
<OAI-PMH xmlns="http://www.openarchives.org/OAI/2.0/">
  <GetRecord><record>
    <header>
      <identifier>oai:eprint.iacr.org:2023/525</identifier>
      <datestamp>2023-04-11T20:49:58Z</datestamp>
    </header>
    <metadata><oai_dc:dc xmlns:oai_dc="http://www.openarchives.org/OAI/2.0/oai_dc/" xmlns:dc="http://purl.org/dc/elements/1.1/">
      <dc:title>Error Correction and Ciphertext Quantization</dc:title>
      <dc:creator>Alice</dc:creator>
      <dc:description>An interesting abstract.</dc:description>
      <dc:date>2023-04-11T20:49:58Z</dc:date>
    </oai_dc:dc></metadata>
  </record></GetRecord>
</OAI-PMH>"##;

    #[test]
    fn parses_get_record() {
        let r = parse_record(GETRECORD_SAMPLE).unwrap().unwrap();
        assert_eq!(r.datestamp, "2023-04-11T20:49:58Z");
        assert_eq!(
            r.title.as_deref(),
            Some("Error Correction and Ciphertext Quantization")
        );
        assert_eq!(r.abstract_.as_deref(), Some("An interesting abstract."));
    }

    #[test]
    fn get_record_id_does_not_exist_is_none() {
        let xml = r##"<OAI-PMH><error code="idDoesNotExist">no such id</error></OAI-PMH>"##;
        assert!(parse_record(xml).unwrap().is_none());
    }

    #[test]
    fn get_record_other_error_propagates() {
        let xml = r##"<OAI-PMH><error code="badArgument">nope</error></OAI-PMH>"##;
        assert!(parse_record(xml).is_err());
    }

    #[test]
    fn rejects_non_eprint_identifier() {
        assert!(parse_oai_identifier("oai:arxiv.org:2024.0001").is_none());
        assert_eq!(
            parse_oai_identifier("oai:eprint.iacr.org:2024/463"),
            Some(PaperId {
                year: 2024,
                num: 463
            })
        );
        assert!(parse_oai_identifier("oai:eprint.iacr.org:2024/463x").is_none());
    }

    /// A realistic record: the header is followed by a <setSpec>, the
    /// metadata has its own dc:identifier, and an <about> provenance block
    /// carries a second <datestamp>. Only the header's values count.
    #[test]
    fn only_header_identifier_and_datestamp_count() {
        let xml = r##"<OAI-PMH><ListRecords><record>
          <header>
            <identifier>oai:eprint.iacr.org:2024/463</identifier>
            <datestamp>2025-01-06T17:43:48Z</datestamp>
            <setSpec>eprint</setSpec>
          </header>
          <metadata><dc><identifier>oai:eprint.iacr.org:1999/001</identifier></dc></metadata>
          <about><provenance><originDescription>
            <identifier>oai:eprint.iacr.org:1999/002</identifier>
            <datestamp>1999-01-01T00:00:00Z</datestamp>
          </originDescription></provenance></about>
        </record></ListRecords></OAI-PMH>"##;
        let p = parse_page(xml).unwrap();
        assert_eq!(
            p.records,
            vec![RecordHeader {
                id: PaperId {
                    year: 2024,
                    num: 463
                },
                datestamp: "2025-01-06T17:43:48Z".into(),
            }]
        );
    }

    /// A header missing its datestamp is skipped; it must not inherit the
    /// previous record's trailing <datestamp> (from its provenance block).
    #[test]
    fn values_do_not_leak_from_one_record_into_the_next() {
        let xml = r##"<OAI-PMH><ListRecords>
          <record><header>
            <identifier>oai:eprint.iacr.org:2024/463</identifier>
            <datestamp>2025-01-06T17:43:48Z</datestamp></header>
            <about><provenance><datestamp>1999-01-01T00:00:00Z</datestamp></provenance></about>
          </record>
          <record><header>
            <identifier>oai:eprint.iacr.org:2024/464</identifier></header>
          </record>
        </ListRecords></OAI-PMH>"##;
        let ids: Vec<String> = parse_page(xml)
            .unwrap()
            .records
            .iter()
            .map(|r| r.id.canonical())
            .collect();
        assert_eq!(ids, ["2024/463"]);
    }

    #[test]
    fn last_page_empty_resumption_token_means_done() {
        let xml = r##"<OAI-PMH><ListRecords>
          <resumptionToken completeListSize="2" cursor="1"></resumptionToken>
        </ListRecords></OAI-PMH>"##;
        assert_eq!(parse_page(xml).unwrap().resumption_token, None);
        let self_closing = r##"<OAI-PMH><ListRecords><resumptionToken/></ListRecords></OAI-PMH>"##;
        assert_eq!(parse_page(self_closing).unwrap().resumption_token, None);
    }

    #[test]
    fn page_errors_as_start_elements() {
        let no_match = r##"<OAI-PMH><error code="noRecordsMatch">none</error></OAI-PMH>"##;
        assert!(parse_page(no_match).unwrap().no_records_match);
        let bad = r##"<OAI-PMH><error code="badResumptionToken">expired</error></OAI-PMH>"##;
        assert!(parse_page(bad)
            .unwrap_err()
            .to_string()
            .contains("badResumptionToken"));
        let bad_empty = r##"<OAI-PMH><error code="badArgument"/></OAI-PMH>"##;
        assert!(parse_page(bad_empty).is_err());
    }

    #[test]
    fn malformed_xml_is_an_error() {
        assert!(parse_page("<OAI-PMH><ListRecords></OAI-PMH>").is_err());
        assert!(parse_record("<OAI-PMH><GetRecord></OAI-PMH>").is_err());
    }

    #[test]
    fn get_record_self_closing_error() {
        let xml = r##"<OAI-PMH><error code="idDoesNotExist"/></OAI-PMH>"##;
        assert!(parse_record(xml).unwrap().is_none());
        let bad = r##"<OAI-PMH><error code="badArgument"/></OAI-PMH>"##;
        assert!(parse_record(bad).is_err());
    }

    /// First title/description win; an empty element doesn't swallow the
    /// next element's text; the header datestamp beats provenance ones.
    #[test]
    fn get_record_field_boundaries() {
        let xml = r##"<OAI-PMH><GetRecord><record>
          <header><identifier>oai:eprint.iacr.org:2023/525</identifier>
            <datestamp>2023-04-11T20:49:58Z</datestamp></header>
          <metadata><dc>
            <title></title><creator>Alice</creator>
            <description>First abstract.</description><description>Second.</description>
          </dc></metadata>
          <about><provenance><datestamp>1999-01-01T00:00:00Z</datestamp></provenance></about>
        </record></GetRecord></OAI-PMH>"##;
        let r = parse_record(xml).unwrap().unwrap();
        assert_eq!(r.datestamp, "2023-04-11T20:49:58Z");
        assert_eq!(r.title, None);
        assert_eq!(r.abstract_.as_deref(), Some("First abstract."));
    }

    #[test]
    fn titles_first_one_wins() {
        let xml = r##"<OAI-PMH><GetRecord><record><header>
          <datestamp>2023-04-11T20:49:58Z</datestamp></header>
          <metadata><dc><title>Main</title><title>Alternative</title></dc></metadata>
        </record></GetRecord></OAI-PMH>"##;
        assert_eq!(
            parse_record(xml).unwrap().unwrap().title.as_deref(),
            Some("Main")
        );
    }
}
