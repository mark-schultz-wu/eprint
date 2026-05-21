//! HTTP client for eprint.iacr.org with polite throttling.
//!
//! State (timestamp of last outbound request) lives in a file under the
//! cache root so it survives restarts and is shared across concurrent
//! invocations.

use anyhow::{Context, Result};
use bytes::Bytes;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{debug, info_span, Instrument};

/// State file under the cache root.
pub const RATE_LIMIT_STAMP: &str = ".rate_limit_stamp";

/// Build the User-Agent string. Academic archives commonly whitelist
/// identifiable traffic; anonymous scrapers are not.
pub fn user_agent(contact: Option<&str>) -> String {
    let base = concat!(
        "eprint/",
        env!("CARGO_PKG_VERSION"),
        " (+https://github.com/mark-schultz-wu/eprint)"
    );
    match contact {
        Some(c) => format!("{base} {c}"),
        None => base.to_owned(),
    }
}

/// Construct a [`reqwest::Client`] with a polite UA and reasonable timeout.
pub fn client(contact: Option<&str>) -> Result<reqwest::Client> {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::USER_AGENT,
        user_agent(contact).parse().context("invalid User-Agent header")?,
    );
    reqwest::Client::builder()
        .default_headers(headers)
        .timeout(Duration::from_secs(120))
        .build()
        .context("building HTTP client")
}

/// Polite rate limiter. Reads / writes the last-request timestamp from
/// `state_path`, sleeps if necessary to honor `min_interval`.
pub struct RateLimiter {
    state_path: PathBuf,
    min_interval: Duration,
}

impl RateLimiter {
    pub fn new(state_path: PathBuf, min_interval_s: f64) -> Self {
        Self {
            state_path,
            min_interval: Duration::from_secs_f64(min_interval_s.max(0.0)),
        }
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
        let now = unix_secs_f();
        let wait = (last + self.min_interval.as_secs_f64()) - now;
        if wait > 0.0 {
            debug!(wait_s = wait, "rate-limit sleep");
            tokio::time::sleep(Duration::from_secs_f64(wait)).await;
        }
        tokio::fs::write(&self.state_path, format!("{:.3}", unix_secs_f())).await?;
        Ok(())
    }
}

fn unix_secs_f() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Fetch a URL as bytes, with rate-limit + polite UA.
pub async fn get_bytes(
    client: &reqwest::Client,
    rl: &RateLimiter,
    url: &str,
) -> Result<Bytes> {
    let span = info_span!("http_get", %url);
    async {
        rl.wait_then_stamp().await?;
        let resp = client
            .get(url)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let retry = resp
                .headers()
                .get("Retry-After")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("unknown");
            anyhow::bail!("eprint.iacr.org returned 429 (Retry-After: {retry}); back off and try again later");
        }
        let resp = resp.error_for_status().with_context(|| format!("GET {url}"))?;
        Ok(resp.bytes().await?)
    }
    .instrument(span)
    .await
}

/// Fetch a URL as a UTF-8 string.
pub async fn get_text(
    client: &reqwest::Client,
    rl: &RateLimiter,
    url: &str,
) -> Result<String> {
    let bytes = get_bytes(client, rl, url).await?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Quick "looks like a PDF" sniff for download validation.
pub fn looks_like_pdf(b: &[u8]) -> bool {
    b.starts_with(b"%PDF")
}

/// Tiny helper to assemble the rate-limiter state path under a cache root.
pub fn rate_limit_path(cache_root: &Path) -> PathBuf {
    cache_root.join(RATE_LIMIT_STAMP)
}
