//! HTTP client for eprint.iacr.org with polite throttling.
//!
//! State (timestamp of last outbound request) lives in a file under the
//! cache root so it survives restarts and is shared across concurrent
//! invocations.

use anyhow::Result;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// State file used by [`RateLimiter`] under `<cache_root>/.rate_limit_stamp`.
pub const RATE_LIMIT_STAMP: &str = ".rate_limit_stamp";

/// Build the User-Agent string. Academic archives commonly whitelist
/// identifiable traffic; anonymous scrapers are not.
pub fn user_agent(contact: Option<&str>) -> String {
    let base = concat!("eprint/", env!("CARGO_PKG_VERSION"), " (+https://github.com/fabriccryptography/eprint)");
    match contact {
        Some(c) => format!("{base} {c}"),
        None => base.to_owned(),
    }
}

/// Polite rate limiter. Reads / writes the last-request timestamp from
/// `state_path`, sleeps if necessary to honor `min_interval`.
pub struct RateLimiter {
    state_path: PathBuf,
    min_interval: Duration,
}

impl RateLimiter {
    pub fn new(state_path: PathBuf, min_interval_s: f64) -> Self {
        Self { state_path, min_interval: Duration::from_secs_f64(min_interval_s.max(0.0)) }
    }

    /// Sleep until the next outbound request is allowed, then stamp.
    pub async fn wait_then_stamp(&self) -> Result<()> {
        if let Some(parent) = self.state_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let last = tokio::fs::read_to_string(&self.state_path)
            .await
            .ok()
            .and_then(|s| s.trim().parse::<f64>().ok())
            .unwrap_or(0.0);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        let wait = (last + self.min_interval.as_secs_f64()) - now;
        if wait > 0.0 {
            tokio::time::sleep(Duration::from_secs_f64(wait)).await;
        }
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        tokio::fs::write(&self.state_path, format!("{stamp:.3}")).await?;
        Ok(())
    }
}
