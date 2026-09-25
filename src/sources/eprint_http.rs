//! Direct PDF fetch from eprint.iacr.org.

use super::{PdfRequest, PdfSource};
use crate::iacr::http;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

/// Direct fetch from eprint.iacr.org by exact version:
/// `/archive/<year>/<num>/<unix-seconds>.pdf`. This serves the current
/// version too (byte-identical to `/<year>/<num>.pdf`), and unlike that
/// "latest" URL it can't hand back a different version than the one asked
/// for, which is what let a stale `current_version` file the newest PDF
/// under an old version. The host rate-limits per IP; `http::get_bytes`
/// backs off and retries on 429.
pub struct EprintHttpSource {
    client: reqwest::Client,
    rl: Arc<http::RateLimiter>,
}

impl EprintHttpSource {
    pub fn new(client: reqwest::Client, rl: Arc<http::RateLimiter>) -> Self {
        Self { client, rl }
    }
}

#[async_trait]
impl PdfSource for EprintHttpSource {
    fn name(&self) -> &'static str {
        "eprint-http"
    }
    fn is_network(&self) -> bool {
        true
    }
    async fn fetch(&self, req: &PdfRequest<'_>) -> Result<Option<Vec<u8>>> {
        let bytes = http::get_bytes(&self.client, &self.rl, &pdf_url(req)).await?;
        Ok(Some(bytes.to_vec()))
    }
}

fn pdf_url(req: &PdfRequest<'_>) -> String {
    req.id.historical_pdf_url(req.version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::PaperId;

    /// Regression: every version, the current one included, is fetched by
    /// its timestamp, so the bytes always match the version directory
    /// they're saved in.
    #[test]
    fn fetches_by_exact_version() {
        let version = "20250106T174348Z".parse().unwrap();
        let req = PdfRequest {
            id: PaperId {
                year: 2024,
                num: 463,
            },
            version: &version,
        };
        assert_eq!(
            pdf_url(&req),
            "https://eprint.iacr.org/archive/2024/463/1736185428.pdf"
        );
    }
}
