//! `eprint sync` — OAI-PMH bulk annotation.
//!
//! For every paper in the cache that appears in OAI-PMH `ListRecords?from=X`,
//! we convert the OAI datestamp to canonical form and add it to
//! `known_versions`. If the canonical timestamp is newer than
//! `current_version`, the paper is implicitly stale (no separate flag).
//!
//! Annotate-only: never downloads PDFs. Next `eprint <id>` notices the
//! newer known_version and pulls if it needs to.

use crate::cache;
use crate::cli::{Context, SyncArgs};
use crate::net;
use crate::oai;
use crate::version;
use anyhow::Result;
use std::path::Path;
use time::macros::format_description;
use time::OffsetDateTime;
use tracing::info;

pub const LAST_SYNC_STAMP: &str = ".last_sync_unix_s";

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

    let client = net::client(cx.cfg.network.contact.as_deref())?;
    let records = oai::list_records(&client, &cx.rate_limiter, Some(&from)).await?;

    let mut updated = 0usize;
    for rec in &records {
        let Some(mut paper_meta) = cache::read_paper_meta(root, rec.id).await else {
            continue;
        };
        let oai: version::OaiDatestamp = rec.datestamp.parse()?;
        let canonical: version::Canonical = (&oai).into();
        if !paper_meta.known_versions.contains(&canonical) {
            paper_meta.known_versions.push(canonical);
            paper_meta.known_versions.sort();
            paper_meta.known_versions.dedup();
            cache::write_paper_meta(root, rec.id, &paper_meta).await?;
            updated += 1;
        }
    }

    let now = now_unix();
    write_last_sync(root, now).await?;
    Ok(SyncReport {
        from,
        records_seen: records.len(),
        cached_papers_updated: updated,
        last_sync_unix_s: now,
    })
}

fn cache_has_any_paper(root: &Path) -> bool {
    let Ok(rd) = std::fs::read_dir(root) else { return false };
    for year in rd.flatten() {
        if !year.file_name().to_string_lossy().chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let Ok(num_rd) = std::fs::read_dir(year.path()) else { continue };
        for paper in num_rd.flatten() {
            if paper.path().join(cache::files::PAPER_META).exists() {
                return true;
            }
        }
    }
    false
}

async fn read_last_sync(root: &Path) -> Option<i64> {
    let s = tokio::fs::read_to_string(root.join(LAST_SYNC_STAMP)).await.ok()?;
    s.trim().parse().ok()
}

async fn effective_from(root: &Path, explicit: Option<&str>, default_window_days: u32) -> String {
    if let Some(s) = explicit {
        return s.to_owned();
    }
    if let Some(unix) = read_last_sync(root).await {
        return iso_date_from_unix(unix);
    }
    let now = now_unix();
    let back = (default_window_days as i64) * 86_400;
    iso_date_from_unix(now - back)
}

async fn write_last_sync(root: &Path, unix_s: i64) -> Result<()> {
    tokio::fs::create_dir_all(root).await?;
    tokio::fs::write(root.join(LAST_SYNC_STAMP), unix_s.to_string()).await?;
    Ok(())
}

fn now_unix() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

fn iso_date_from_unix(unix_s: i64) -> String {
    let dt = OffsetDateTime::from_unix_timestamp(unix_s.max(0))
        .unwrap_or(OffsetDateTime::UNIX_EPOCH);
    dt.format(format_description!("[year]-[month]-[day]"))
        .expect("YYYY-MM-DD format is infallible")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn date_math_known_points() {
        assert_eq!(iso_date_from_unix(0), "1970-01-01");
        assert_eq!(iso_date_from_unix(1_779_321_600), "2026-05-21");
        assert_eq!(iso_date_from_unix(1_709_164_800), "2024-02-29");
    }
}
