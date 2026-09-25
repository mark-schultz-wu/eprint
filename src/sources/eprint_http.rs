//! Direct PDF fetch from eprint.iacr.org.

use super::{PdfRequest, PdfSource};
use crate::iacr::http;
use crate::iacr::site::Site;
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
    site: Site,
}

impl EprintHttpSource {
    pub fn new(client: reqwest::Client, rl: Arc<http::RateLimiter>, site: Site) -> Self {
        Self { client, rl, site }
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
        let url = self.site.version_pdf_url(req.id, req.version);
        let bytes = http::get_bytes(&self.client, &self.rl, &url).await?;
        Ok(Some(bytes.to_vec()))
    }
}
