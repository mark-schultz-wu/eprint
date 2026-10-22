//! The current PDF from a host without the version archive (the default
//! mirror): `/<year>/<num>.pdf`.

use super::{PdfRequest, PdfSource};
use crate::iacr::http;
use crate::iacr::site::Site;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

/// Fetches `/<year>/<num>.pdf`, which is whatever version is current now.
///
/// It answers only when the request is for the paper's current version
/// (`Ok(None)` otherwise), so it can never file one version's bytes under
/// another version's label. The current version comes from OAI-PMH on the
/// same host just before the fetch; see
/// [`crate::commands::paper`]'s `known_versions::refresh`.
pub struct EprintCurrentSource {
    client: reqwest::Client,
    rl: Arc<http::RateLimiter>,
    site: Site,
}

impl EprintCurrentSource {
    pub fn new(client: reqwest::Client, rl: Arc<http::RateLimiter>, site: Site) -> Self {
        Self { client, rl, site }
    }
}

#[async_trait]
impl PdfSource for EprintCurrentSource {
    fn name(&self) -> &'static str {
        "eprint-current"
    }
    fn is_network(&self) -> bool {
        true
    }
    async fn fetch(&self, req: &PdfRequest<'_>) -> Result<Option<Vec<u8>>> {
        if !req.is_current() {
            return Ok(None);
        }
        let bytes =
            http::get_bytes(&self.client, &self.rl, &self.site.current_pdf_url(req.id)).await?;
        Ok(Some(bytes.to_vec()))
    }
}
