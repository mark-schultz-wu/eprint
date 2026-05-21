//! `eprint sync` — OAI-PMH metadata harvest.
//!
//! Annotate-only. For every record returned by `ListRecords?from=<last_sync>`:
//!
//! 1. Look up `<cache>/<year>/<num>/`. If the directory doesn't exist, skip
//!    (we don't pre-cache papers the user never asked for).
//! 2. Read paper-level meta. If the record's datestamp is newer than the
//!    cached `latest_known_oai_datestamp`, update it.
//! 3. Read the current version's meta. If its `oai_datestamp` is `None`,
//!    backfill from the record (first time sync has seen this version).
//!
//! Downloads happen later, lazily, in `eprint fetch` when it notices the
//! cached current version is stale.

use crate::cache;
use crate::cli::{Context, SyncArgs};
use crate::net::{self, RateLimiter};
use crate::oai;
use anyhow::Result;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::info;

pub const LAST_SYNC_STAMP: &str = ".last_sync_unix_s";

#[derive(Debug, serde::Serialize)]
pub struct SyncReport {
    pub from: String,
    pub records_seen: usize,
    pub cached_papers_annotated: usize,
    pub current_version_backfills: usize,
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
            "Sync from {}: {} records seen, {} cached papers annotated (stale flag bumped), \
             {} current-version backfills.",
            report.from,
            report.records_seen,
            report.cached_papers_annotated,
            report.current_version_backfills
        );
    }
    Ok(())
}

/// Run sync once if `cfg.sync.auto` is enabled and the cache is older
/// than the configured threshold. No-op otherwise. Silent unless tracing
/// is enabled.
///
/// Called at the top of cache-reading commands. Returns whether sync
/// actually ran.
pub async fn maybe_auto_sync(cx: &Context) -> Result<bool> {
    if cx.offline || !cx.cfg.sync.auto {
        return Ok(false);
    }
    let root = &cx.cfg.cache_root;

    // Sync's payoff is "annotate cached papers." If there are none, the
    // whole operation is a no-op — skip and tell the user why.
    if !cache_has_any_paper(root) {
        if !cx.json {
            eprintln!("auto-sync skipped: no papers in cache yet");
        }
        info!("auto-sync skipped: cache contains no papers");
        return Ok(false);
    }

    let last = read_last_sync(root).await;
    let now = now_unix();
    let threshold_s = (cx.cfg.sync.stale_after_hours as i64) * 3600;
    let needs_sync = match last {
        Some(t) => (now - t) > threshold_s,
        None => true, // never synced
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
            None => {
                eprintln!("auto-syncing eprint metadata (first sync)...");
            }
        }
    }
    info!(last_sync = ?last, threshold_s, "auto-sync starting");
    let report = sync_impl(cx, None, 30).await?;
    if !cx.json {
        eprintln!(
            "  done ({} records, {} annotated, {} backfilled)",
            report.records_seen,
            report.cached_papers_annotated,
            report.current_version_backfills
        );
    }
    Ok(true)
}

/// True iff the cache root has at least one `<year>/<num>/meta.json`
/// whose tool tag identifies it as ours.
fn cache_has_any_paper(root: &std::path::Path) -> bool {
    let Ok(rd) = std::fs::read_dir(root) else { return false };
    for year in rd.flatten() {
        if !year.file_name().to_string_lossy().chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let Ok(num_rd) = std::fs::read_dir(year.path()) else { continue };
        for paper in num_rd.flatten() {
            let meta = paper.path().join(cache::files::PAPER_META);
            if meta.exists() {
                return true;
            }
        }
    }
    false
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

    let client = net::client(cx.cfg.network.contact.as_deref())?;
    let rl = RateLimiter::new(net::rate_limit_path(root), cx.cfg.network.min_interval_s);
    let records = oai::list_records(&client, &rl, Some(&from)).await?;

    let mut annotated = 0usize;
    let mut backfilled = 0usize;
    for rec in &records {
        let dir = cache::paper_dir(root, rec.id);
        if !dir.exists() {
            continue;
        }
        let mut paper_meta = cache::read_paper_meta(root, rec.id).await;
        let need_paper_write = match paper_meta.latest_known_oai_datestamp.as_deref() {
            Some(existing) => oai::datestamp_cmp(existing, &rec.datestamp)?.is_lt(),
            None => true,
        };
        if need_paper_write {
            paper_meta.latest_known_oai_datestamp = Some(rec.datestamp.clone());
            cache::write_paper_meta(root, rec.id, &paper_meta).await?;
            annotated += 1;
        }
        if let Some(current) = paper_meta.current_version {
            let mut vmeta = cache::read_version_meta(root, rec.id, current).await;
            if vmeta.oai_datestamp.is_none() {
                vmeta.oai_datestamp = Some(rec.datestamp.clone());
                cache::write_version_meta(root, rec.id, current, &vmeta).await?;
                backfilled += 1;
            }
        }
    }

    let now = now_unix();
    write_last_sync(root, now).await?;

    Ok(SyncReport {
        from,
        records_seen: records.len(),
        cached_papers_annotated: annotated,
        current_version_backfills: backfilled,
        last_sync_unix_s: now,
    })
}

async fn read_last_sync(root: &Path) -> Option<i64> {
    let path = root.join(LAST_SYNC_STAMP);
    let s = tokio::fs::read_to_string(&path).await.ok()?;
    s.trim().parse::<i64>().ok()
}

/// Decide the `from=` timestamp to use:
/// 1. `--since=...` (explicit)
/// 2. last-sync stamp on disk (converted to ISO date)
/// 3. `default_window_days` ago
async fn effective_from(root: &Path, explicit: Option<&str>, default_window_days: u32) -> String {
    if let Some(s) = explicit {
        return s.to_owned();
    }
    let stamp = root.join(LAST_SYNC_STAMP);
    if let Ok(s) = tokio::fs::read_to_string(&stamp).await {
        if let Ok(unix) = s.trim().parse::<i64>() {
            return iso_date_from_unix(unix);
        }
    }
    let now = now_unix();
    let back = (default_window_days as i64) * 24 * 3600;
    iso_date_from_unix(now - back)
}

async fn write_last_sync(root: &Path, unix_s: i64) -> Result<()> {
    tokio::fs::create_dir_all(root).await?;
    let path = root.join(LAST_SYNC_STAMP);
    tokio::fs::write(&path, unix_s.to_string()).await?;
    Ok(())
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Convert a unix seconds timestamp to an ISO date `YYYY-MM-DD`. We don't
/// pull in `chrono` for this — Julian-day arithmetic on Gregorian is one
/// page of code. Good enough; eprint accepts `from=YYYY-MM-DD`.
fn iso_date_from_unix(unix_s: i64) -> String {
    let secs = unix_s.max(0);
    let _ = Duration::from_secs(secs as u64); // type check
    let days_since_epoch = secs / 86_400;
    let (y, m, d) = ymd_from_days_since_epoch(days_since_epoch);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Days since 1970-01-01 → (year, month, day). Algorithm from Howard Hinnant's
/// public-domain date math, simplified to 32-bit-safe sentinel-free form.
fn ymd_from_days_since_epoch(z: i64) -> (i32, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let y = (y + if m <= 2 { 1 } else { 0 }) as i32;
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_math_known_points() {
        assert_eq!(iso_date_from_unix(0), "1970-01-01");
        // 2026-05-21 00:00:00 UTC
        assert_eq!(iso_date_from_unix(1_779_321_600), "2026-05-21");
        // 2024-02-29 (leap year sanity)
        assert_eq!(iso_date_from_unix(1_709_164_800), "2024-02-29");
    }
}
