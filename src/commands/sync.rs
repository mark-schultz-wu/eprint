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
use crate::iacr::oai;
use crate::ids::version;
use anyhow::Result;
use std::path::Path;
use time::macros::format_description;
use time::{Duration, OffsetDateTime};
use tracing::{info, warn};

/// How far back the first sync looks, absent `--since` or a previous sync.
pub const DEFAULT_WINDOW_DAYS: u32 = 30;

#[derive(Debug, serde::Serialize)]
pub struct SyncReport {
    pub from: String,
    pub records_seen: usize,
    pub cached_papers_updated: usize,
    #[serde(with = "time::serde::rfc3339")]
    pub last_sync: OffsetDateTime,
}

pub async fn run(cx: &Context, args: SyncArgs) -> Result<()> {
    if cx.offline {
        anyhow::bail!("--offline set; sync requires network");
    }
    let window = Duration::days(args.default_window_days.into());
    let report = sync_impl(cx, args.since.as_deref(), window).await?;
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
    let now = OffsetDateTime::now_utc();
    if !sync_due(now, last, cx.cfg.sync.stale_after) {
        return Ok(false);
    }
    if !cx.json {
        match last {
            Some(t) => eprintln!(
                "auto-syncing eprint metadata (last sync: {}h ago)...",
                (now - t).whole_hours()
            ),
            None => eprintln!("auto-syncing eprint metadata (first sync)..."),
        }
    }
    info!(last_sync = ?last, "auto-sync starting");
    let report = sync_impl(cx, None, Duration::days(DEFAULT_WINDOW_DAYS.into())).await?;
    if !cx.json {
        eprintln!(
            "  done ({} records, {} cached papers updated)",
            report.records_seen, report.cached_papers_updated
        );
    }
    Ok(true)
}

async fn sync_impl(cx: &Context, since: Option<&str>, window: Duration) -> Result<SyncReport> {
    let root = &cx.cfg.cache_root;
    tokio::fs::create_dir_all(root).await?;

    let last = cache::read_last_sync(root).await;
    let from = from_date(OffsetDateTime::now_utc(), since, last, window);
    info!(from = %from, "starting OAI-PMH sync");

    let records =
        oai::list_records(&cx.http, &cx.rate_limiter, &cx.site.oai_url(), Some(&from)).await?;

    let updated = apply_records(root, &records).await?;

    let now = OffsetDateTime::now_utc();
    cache::write_last_sync(root, now).await?;
    Ok(SyncReport {
        from,
        records_seen: records.len(),
        cached_papers_updated: updated,
        last_sync: now,
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

/// Whether an auto-sync is due: never synced, or the last sync is older
/// than `stale_after`.
fn sync_due(now: OffsetDateTime, last: Option<OffsetDateTime>, stale_after: Duration) -> bool {
    last.is_none_or(|t| now - t > stale_after)
}

/// The `from` date for `ListRecords`: an explicit `--since`, else the day of
/// the last sync, else `window` before now.
fn from_date(
    now: OffsetDateTime,
    explicit: Option<&str>,
    last: Option<OffsetDateTime>,
    window: Duration,
) -> String {
    if let Some(s) = explicit {
        return s.to_owned();
    }
    let start = last.unwrap_or(now - window);
    start
        .format(format_description!("[year]-[month]-[day]"))
        .expect("YYYY-MM-DD format is infallible")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::PaperId;
    use time::macros::datetime;

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

    const NOW: OffsetDateTime = datetime!(2026-05-21 00:00 UTC);

    #[test]
    fn sync_is_due_when_never_run_or_older_than_the_threshold() {
        let day = Duration::hours(24);
        assert!(sync_due(NOW, None, day));
        assert!(!sync_due(NOW, Some(NOW - Duration::hours(1)), day));
        assert!(
            !sync_due(NOW, Some(NOW - day), day),
            "exactly at the threshold"
        );
        assert!(sync_due(NOW, Some(NOW - day - Duration::seconds(1)), day));
        assert!(sync_due(
            NOW,
            Some(NOW - Duration::hours(2)),
            Duration::hours(1)
        ));
    }

    #[test]
    fn from_date_prefers_explicit_then_last_sync_then_window() {
        let month = Duration::days(30);
        let yesterday = NOW - Duration::days(1);
        assert_eq!(
            from_date(NOW, Some("2020-01-01"), Some(yesterday), month),
            "2020-01-01"
        );
        assert_eq!(from_date(NOW, None, Some(yesterday), month), "2026-05-20");
        assert_eq!(from_date(NOW, None, None, month), "2026-04-21");
        assert_eq!(from_date(NOW, None, None, Duration::days(1)), "2026-05-20");
        let leap = datetime!(2024-02-29 23:59:59 UTC);
        assert_eq!(from_date(NOW, None, Some(leap), month), "2024-02-29");
    }
}
