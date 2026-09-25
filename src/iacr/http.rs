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
use tracing::{debug, info, info_span, warn, Instrument};

pub type RateLimiter = Governor<NotKeyed, InMemoryState, DefaultClock>;

/// Build a fresh `Arc<RateLimiter>` for this process's lifetime.
/// `interval` is the sustained period per request (clamped to at least
/// 1 ms); `burst` is how many tokens the bucket can hold.
pub fn rate_limiter(interval: Duration, burst: u32) -> Arc<RateLimiter> {
    let period = interval.max(Duration::from_millis(1));
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
        user_agent(contact)
            .parse()
            .context("invalid User-Agent header")?,
    );
    reqwest::Client::builder()
        .default_headers(headers)
        .timeout(Duration::from_secs(120))
        .build()
        .context("building HTTP client")
}

/// Waits before each retry of a 429'd request; its length is the retry budget.
///
/// eprint.iacr.org rate-limits per IP: roughly 20 back-to-back requests draw a
/// 429 with no `Retry-After` header. Observed recovery ranged from under 10 s
/// to ~1–2 min (consistent with a sliding window of about a minute), so the
/// schedule escalates to cover the slow case: 105 s total before giving up.
const RETRY_BACKOFFS: [Duration; 3] = [
    Duration::from_secs(15),
    Duration::from_secs(30),
    Duration::from_secs(60),
];

/// Upper bound on a server-supplied `Retry-After`, so a hostile or buggy
/// header can't park us for hours.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(300);

/// How hard to retry a rate-limited (429) request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retry {
    /// Wait out the rate limit ([`RETRY_BACKOFFS`], up to ~105 s): for
    /// things the user asked for.
    Patient,
    /// Give up at once: for background work (auto-sync) that mustn't stall
    /// the command the user actually ran.
    Quick,
}

impl Retry {
    fn schedule(self) -> &'static [Duration] {
        match self {
            Retry::Patient => &RETRY_BACKOFFS,
            Retry::Quick => &[],
        }
    }
}

/// How long to wait before retry number `attempt` (0-based) under `retry`,
/// or `None` once the budget is spent. A `Retry-After` in delta-seconds form
/// wins over the fixed schedule (capped at [`MAX_RETRY_AFTER`]); the
/// HTTP-date form is ignored in favour of the schedule.
fn retry_delay(retry: Retry, attempt: usize, retry_after: Option<&str>) -> Option<Duration> {
    let fallback = *retry.schedule().get(attempt)?;
    let server = retry_after
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(|s| Duration::from_secs(s).min(MAX_RETRY_AFTER));
    Some(server.unwrap_or(fallback))
}

/// Fetch a URL as bytes with [`Retry::Patient`].
pub async fn get_bytes(client: &reqwest::Client, rl: &RateLimiter, url: &str) -> Result<Bytes> {
    get_bytes_with(client, rl, url, Retry::Patient).await
}

/// Fetch a URL as bytes, blocking until the rate limiter grants a token.
///
/// A 429 is retried per `retry` (each retry also takes a fresh rate-limiter
/// token); any other error status fails immediately.
pub async fn get_bytes_with(
    client: &reqwest::Client,
    rl: &RateLimiter,
    url: &str,
    retry: Retry,
) -> Result<Bytes> {
    let span = info_span!("http_get", %url);
    async {
        for attempt in 0.. {
            rl.until_ready().await;
            debug!(attempt, "rate limiter granted");
            let resp = client
                .get(url)
                .send()
                .await
                .with_context(|| format!("GET {url}"))?;
            if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                let retry_after = resp
                    .headers()
                    .get("Retry-After")
                    .and_then(|v| v.to_str().ok());
                let budget = retry.schedule().len();
                let Some(delay) = retry_delay(retry, attempt, retry_after) else {
                    anyhow::bail!(
                        "eprint.iacr.org kept returning 429 (rate limited) for {url} after \
                         {budget} retries; wait a minute and re-run, or raise \
                         EPRINT_MIN_INTERVAL_S",
                    );
                };
                warn!(
                    retry_in_s = delay.as_secs(),
                    retry = attempt + 1,
                    of = budget,
                    "eprint.iacr.org rate-limited this request (429); backing off"
                );
                tokio::time::sleep(delay).await;
                continue;
            }
            let resp = resp
                .error_for_status()
                .with_context(|| format!("GET {url}"))?;
            info!(bytes = ?resp.content_length(), "fetched");
            return Ok(resp.bytes().await?);
        }
        unreachable!("retry loop only exits by returning")
    }
    .instrument(span)
    .await
}

/// Fetch a URL as a UTF-8 string with [`Retry::Patient`].
pub async fn get_text(client: &reqwest::Client, rl: &RateLimiter, url: &str) -> Result<String> {
    get_text_with(client, rl, url, Retry::Patient).await
}

/// Fetch a URL as a UTF-8 string.
pub async fn get_text_with(
    client: &reqwest::Client,
    rl: &RateLimiter,
    url: &str,
    retry: Retry,
) -> Result<String> {
    let bytes = get_bytes_with(client, rl, url, retry).await?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Heuristic PDF sniff.
pub fn looks_like_pdf(b: &[u8]) -> bool {
    b.starts_with(b"%PDF")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_never_retries_even_if_the_server_says_when() {
        assert_eq!(retry_delay(Retry::Quick, 0, None), None);
        assert_eq!(retry_delay(Retry::Quick, 0, Some("1")), None);
    }

    #[test]
    fn schedule_without_retry_after_then_gives_up() {
        let got: Vec<_> = (0..=RETRY_BACKOFFS.len())
            .map(|a| retry_delay(Retry::Patient, a, None))
            .collect();
        let mut want: Vec<_> = RETRY_BACKOFFS.iter().copied().map(Some).collect();
        want.push(None);
        assert_eq!(got, want);
    }

    #[test]
    fn delta_seconds_retry_after_wins_and_is_capped() {
        assert_eq!(
            retry_delay(Retry::Patient, 0, Some("7")),
            Some(Duration::from_secs(7))
        );
        assert_eq!(
            retry_delay(Retry::Patient, 0, Some(" 7 ")),
            Some(Duration::from_secs(7))
        );
        assert_eq!(
            retry_delay(Retry::Patient, 1, Some("100000")),
            Some(MAX_RETRY_AFTER)
        );
    }

    #[test]
    fn unparseable_retry_after_falls_back_to_schedule() {
        let http_date = "Fri, 25 Sep 2026 18:49:47 GMT";
        assert_eq!(
            retry_delay(Retry::Patient, 0, Some(http_date)),
            Some(RETRY_BACKOFFS[0])
        );
    }

    #[test]
    fn retry_after_does_not_extend_the_budget() {
        assert_eq!(
            retry_delay(Retry::Patient, RETRY_BACKOFFS.len(), Some("1")),
            None
        );
    }
}
