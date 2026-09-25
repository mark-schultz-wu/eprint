//! Direct PDF fetch from eprint.iacr.org.

use super::{PdfRequest, PdfSource};
use crate::iacr::http;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

/// Direct fetch from eprint.iacr.org: `/<year>/<num>.pdf` for the current
/// version, `/archive/<year>/<num>/<unix-seconds>.pdf` for historical ones.
/// The host rate-limits per IP; `http::get_bytes` backs off and retries on 429.
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
        let url = if req.is_current {
            req.id.pdf_url()
        } else {
            req.id.historical_pdf_url(req.version)
        };
        let bytes = http::get_bytes(&self.client, &self.rl, &url).await?;
        Ok(Some(bytes.to_vec()))
    }
}

