//! HTTP client + token-bucket rate limiter.
//!
//! Rate limiting uses the `governor` crate: a single shared
//! `DefaultDirectRateLimiter` per process, with `burst=3, refill=1/2s`
//! (one request every 2 s sustained, up to 3 in a burst). That's well
//! below anything `eprint.iacr.org` would object to and lets us run a
//! pair of fetches (e.g. archive listing + landing page) concurrently
//! without violating the average rate.
//!
//! In-memory only — no cross-process coordination. Two `eprint`
//! processes running in parallel would each get their own bucket; in
//! practice this is rare enough we accept the brief 2x rate.

use anyhow::{Context as _, Result};
use bytes::Bytes;
use governor::clock::DefaultClock;
use governor::state::{InMemoryState, NotKeyed};
use governor::{Quota, RateLimiter as Governor};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, info_span, Instrument};

pub type RateLimiter = Governor<NotKeyed, InMemoryState, DefaultClock>;

/// Build a fresh `Arc<RateLimiter>` for this process's lifetime.
/// `interval_s` is the sustained period (seconds per request); `burst` is
/// how many tokens the bucket can hold.
pub fn rate_limiter(interval_s: f64, burst: u32) -> Arc<RateLimiter> {
    let period = Duration::from_secs_f64(interval_s.max(0.001));
    let quota = Quota::with_period(period)
        .expect("rate-limit period must be > 0")
        .allow_burst(NonZeroU32::new(burst.max(1)).unwrap());
    Arc::new(Governor::direct(quota))
}

/// Build a polite User-Agent string.
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

/// Construct a polite `reqwest::Client`.
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

/// Fetch a URL as bytes, blocking until the rate limiter grants a token.
pub async fn get_bytes(client: &reqwest::Client, rl: &RateLimiter, url: &str) -> Result<Bytes> {
    let span = info_span!("http_get", %url);
    async {
        rl.until_ready().await;
        debug!("rate limiter granted");
        let resp = client.get(url).send().await.with_context(|| format!("GET {url}"))?;
        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let retry = resp
                .headers()
                .get("Retry-After")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("unknown");
            anyhow::bail!("eprint.iacr.org returned 429 (Retry-After: {retry})");
        }
        let resp = resp.error_for_status().with_context(|| format!("GET {url}"))?;
        info!(bytes = ?resp.content_length(), "fetched");
        Ok(resp.bytes().await?)
    }
    .instrument(span)
    .await
}

/// Fetch a URL as a UTF-8 string.
pub async fn get_text(client: &reqwest::Client, rl: &RateLimiter, url: &str) -> Result<String> {
    let bytes = get_bytes(client, rl, url).await?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Heuristic PDF sniff.
pub fn looks_like_pdf(b: &[u8]) -> bool {
    b.starts_with(b"%PDF")
}
