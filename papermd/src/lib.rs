//! Convert academic PDFs to Markdown.
//!
//! [`Converter`] is the entry point. Two backends behind Cargo features:
//!
//! - `local` ([`LocalConverter`]) — subprocesses MinerU via `uv`.
//! - `remote` ([`RemoteConverter`]) — HTTP client that talks to a server
//!   speaking `POST /v1/convert` (MinerU's FastAPI server in v1).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[cfg(feature = "local")]
pub mod local;
#[cfg(feature = "remote")]
pub mod remote;

#[cfg(feature = "local")]
pub use local::LocalConverter;
#[cfg(feature = "remote")]
pub use remote::RemoteConverter;

/// Quality tier requested from a [`Converter`].
///
/// `Text` is fast and lossy — prose only, no math, no tables.
/// `Ml` is slow but extracts LaTeX math and structured tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Quality {
    Text,
    Ml,
}

/// Output of a successful conversion.
#[derive(Debug, Clone)]
pub struct Conversion {
    /// The extracted Markdown.
    pub markdown: String,
    /// The quality tier the backend actually produced. Backends may
    /// downgrade if the requested tier isn't available.
    pub quality: Quality,
    /// Wall-clock time the backend spent.
    pub duration: std::time::Duration,
}

/// Errors a [`Converter`] can produce.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("backend not available: {0}")]
    BackendUnavailable(String),
    #[error("PDF input not found: {0}")]
    InputNotFound(PathBuf),
    #[error("backend produced no output")]
    NoOutput,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("subprocess failed (exit code {code:?}): {stderr}")]
    Subprocess { code: Option<i32>, stderr: String },
    #[cfg(feature = "remote")]
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("unexpected: {0}")]
    Unexpected(String),
}

/// Result alias for the crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Async PDF-to-Markdown converter.
///
/// Implementations may shell out to external binaries, make HTTP calls,
/// or do the work in-process. Callers should not assume any particular
/// backend behavior beyond "PDF in, Markdown out."
#[async_trait::async_trait]
pub trait Converter: Send + Sync {
    /// Convert a PDF at `pdf_path` to Markdown at the requested quality.
    async fn convert(&self, pdf_path: &std::path::Path, quality: Quality) -> Result<Conversion>;
}
