//! `eprint sync` — OAI-PMH bulk annotation.
//!
//! For every cached paper that appears in OAI-PMH `ListRecords?from=X`, we
//! record the OAI datestamp (its last modification) as the paper's
//! `last_modified` hint. The next `eprint paper <id>` sees the hint is newer
//! than its last archive listing and re-lists, picking up any new version.
//!
//! Annotate-only: never downloads anything but OAI metadata.

use crate::cache;
use crate::cli::{Context, SyncArgs};
use crate::iacr::http;
use crate::iacr::oai;
use crate::ids::version;
use anyhow::Result;
use std::path::Path;
use time::macros::format_description;
use time::OffsetDateTime;
use tracing::{info, warn};

#[derive(Debug, serde::Serialize)]
pub struct SyncReport {
    pub from: String,
    pub records_seen: usize,
    pub cached_papers_updated: usize,
    pub last_sync_unix_s: i64,
}

pub async fn run(cx: &Context, args: SyncArgs) -> Result<()> {
    if cx.offline {
        anyhow::bail!("--offline set; sync requires network");
    }
    let report = sync_impl(cx, args.since.as_deref(), args.default_window_days).await?;
    if cx.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "Sync from {}: {} records seen, {} cached papers updated.",
            report.from, report.records_seen, report.cached_papers_updated
        );
    }
    Ok(())
}

/// Auto-sync hook. Skipped when cache empty, --offline, or auto disabled.
pub async fn maybe_auto_sync(cx: &Context) -> Result<bool> {
    if cx.offline || !cx.cfg.sync.auto {
        return Ok(false);
    }
    let root = &cx.cfg.cache_root;
    if !cache::scan::has_any_paper(root) {
        info!("auto-sync skipped: cache contains no papers");
        return Ok(false);
    }
    let last = cache::read_last_sync(root).await;
    let now = now_unix();
    let threshold_s = (cx.cfg.sync.stale_after_hours as i64) * 3600;
    let needs_sync = match last {
        Some(t) => (now - t) > threshold_s,
        None => true,
    };
    if !needs_sync {
        return Ok(false);
    }
    if !cx.json {
        match last {
            Some(t) => {
                let age_h = (now - t) / 3600;
                eprintln!("auto-syncing eprint metadata (last sync: {age_h}h ago)...");
            }
            None => eprintln!("auto-syncing eprint metadata (first sync)..."),
        }
    }
    info!(last_sync = ?last, threshold_s, "auto-sync starting");
    let report = sync_impl(cx, None, 30).await?;
    if !cx.json {
        eprintln!(
            "  done ({} records, {} cached papers updated)",
            report.records_seen, report.cached_papers_updated
        );
    }
    Ok(true)
}

async fn sync_impl(
    cx: &Context,
    since: Option<&str>,
    default_window_days: u32,
) -> Result<SyncReport> {
    let root = &cx.cfg.cache_root;
    tokio::fs::create_dir_all(root).await?;

    let from = effective_from(root, since, default_window_days).await;
    info!(from = %from, "starting OAI-PMH sync");

    let client = http::client(cx.cfg.network.contact.as_deref())?;
    let records = oai::list_records(&client, &cx.rate_limiter, Some(&from)).await?;

    let updated = apply_records(root, &records).await?;

    let now = now_unix();
    cache::write_last_sync(root, now).await?;
    Ok(SyncReport {
        from,
        records_seen: records.len(),
        cached_papers_updated: updated,
        last_sync_unix_s: now,
    })
}

/// Record each OAI record's datestamp on the matching cached paper (if
/// any). Returns how many papers changed. A record with a malformed
/// datestamp is skipped with a warning rather than failing the whole sync.
async fn apply_records(root: &Path, records: &[oai::RecordHeader]) -> Result<usize> {
    let mut updated = 0;
    for rec in records {
        let Some(mut paper_meta) = cache::read_paper_meta(root, rec.id).await else {
            continue;
        };
        let oai: version::OaiDatestamp = match rec.datestamp.parse() {
            Ok(d) => d,
            Err(e) => {
                warn!(id = %rec.id, error = %e, "skipping OAI record with a malformed datestamp");
                continue;
            }
        };
        if paper_meta.note_modified((&oai).into()) {
            cache::write_paper_meta(root, rec.id, &paper_meta).await?;
            updated += 1;
        }
    }
    Ok(updated)
}

async fn effective_from(root: &Path, explicit: Option<&str>, default_window_days: u32) -> String {
    if let Some(s) = explicit {
        return s.to_owned();
    }
    if let Some(unix) = cache::read_last_sync(root).await {
        return iso_date_from_unix(unix);
    }
    let now = now_unix();
    let back = (default_window_days as i64) * 86_400;
    iso_date_from_unix(now - back)
}

fn now_unix() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

fn iso_date_from_unix(unix_s: i64) -> String {
    let dt =
        OffsetDateTime::from_unix_timestamp(unix_s.max(0)).unwrap_or(OffsetDateTime::UNIX_EPOCH);
    dt.format(format_description!("[year]-[month]-[day]"))
        .expect("YYYY-MM-DD format is infallible")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::PaperId;

    fn rec(year: u16, num: u32, datestamp: &str) -> oai::RecordHeader {
        oai::RecordHeader {
            id: PaperId { year, num },
            datestamp: datestamp.into(),
        }
    }

    /// Regression: one malformed datestamp used to abort the whole sync.
    #[tokio::test]
    async fn malformed_datestamps_are_skipped_not_fatal() {
        let root = tempfile::tempdir().unwrap();
        let cached = [
            PaperId { year: 2024, num: 1 },
            PaperId { year: 2024, num: 2 },
        ];
        for id in cached {
            let v = "20240101T000000Z".parse().unwrap();
            cache::write_paper_meta(root.path(), id, &cache::PaperMeta::for_first_fetch(v))
                .await
                .unwrap();
        }
        let records = [
            rec(2024, 1, "not a datestamp"),
            rec(2024, 2, "2025-01-06T17:43:48Z"),
            rec(2024, 3, "2025-01-06T17:43:48Z"), // not cached: ignored
        ];
        assert_eq!(apply_records(root.path(), &records).await.unwrap(), 1);
        let m = cache::read_paper_meta(root.path(), cached[1])
            .await
            .unwrap();
        assert_eq!(m.last_modified.unwrap().to_string(), "20250106T174348Z");
        assert!(m.needs_listing());
        // Re-applying the same records changes nothing.
        assert_eq!(apply_records(root.path(), &records).await.unwrap(), 0);
    }

    #[test]
    fn date_math_known_points() {
        assert_eq!(iso_date_from_unix(0), "1970-01-01");
        assert_eq!(iso_date_from_unix(1_779_321_600), "2026-05-21");
        assert_eq!(iso_date_from_unix(1_709_164_800), "2024-02-29");
    }
}
