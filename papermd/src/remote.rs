//! Remote backend: HTTP client.
//!
//! Talks to any server exposing `POST /v1/convert`. In v1 this is the
//! MinerU FastAPI server bundled with the `mineru` CLI; in v2 it will
//! be a standalone `papermd-server` binary. Same wire protocol either way.

use crate::{Conversion, Converter, Error, Quality, Result};
use std::path::Path;
use std::time::{Duration, Instant};
use tracing::{debug, info_span, Instrument};

/// HTTP-backed [`Converter`].
pub struct RemoteConverter {
    /// Base URL, e.g. `https://mineru.fabriccrypto.internal`.
    pub endpoint: String,
    /// Optional bearer token.
    pub token: Option<String>,
    client: reqwest::Client,
}

impl RemoteConverter {
    /// Build a new remote converter with a 20-minute request timeout.
    /// ML conversion of a math-heavy paper takes 5–10 min, so don't go
    /// below 10–15 minutes.
    pub fn new(endpoint: impl Into<String>) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(20 * 60))
            .build()?;
        Ok(Self { endpoint: endpoint.into(), token: None, client })
    }

    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }
}

#[async_trait::async_trait]
impl Converter for RemoteConverter {
    async fn convert(&self, pdf_path: &Path, quality: Quality) -> Result<Conversion> {
        if !pdf_path.exists() {
            return Err(Error::InputNotFound(pdf_path.to_path_buf()));
        }
        let span = info_span!("papermd_remote_convert", endpoint = %self.endpoint);
        async {
            let start = Instant::now();
            let pdf_bytes = tokio::fs::read(pdf_path).await?;
            let url = format!("{}/v1/convert", self.endpoint.trim_end_matches('/'));
            let quality_str = match quality {
                Quality::Text => "text",
                Quality::Ml => "ml",
            };
            let mut req = self
                .client
                .post(&url)
                .query(&[("quality", quality_str)])
                .header("Content-Type", "application/pdf")
                .body(pdf_bytes);
            if let Some(token) = &self.token {
                req = req.bearer_auth(token);
            }
            let resp = req.send().await?.error_for_status()?;
            let returned_quality = resp
                .headers()
                .get("X-PaperMD-Quality")
                .and_then(|v| v.to_str().ok())
                .map(parse_quality);
            let markdown = resp.text().await?;
            let duration = start.elapsed();
            debug!(?duration, bytes = markdown.len(), "remote convert completed");
            Ok(Conversion {
                markdown,
                quality: returned_quality.unwrap_or(quality),
                duration,
            })
        }
        .instrument(span)
        .await
    }
}

fn parse_quality(s: &str) -> Quality {
    if s == "ml" { Quality::Ml } else { Quality::Text }
}
