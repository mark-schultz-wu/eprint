//! Local backend: subprocesses MinerU via `uv`.
//!
//! Requires `uv` on `PATH`. The first call will install MinerU into uv's
//! cache (heavy: ~1–2 GB of model weights). Subsequent calls are fast to
//! start up but the conversion itself still takes minutes for math-heavy
//! papers.

use crate::{Conversion, Converter, Error, Quality, Result};
use std::path::{Path, PathBuf};
use std::time::Instant;
use tokio::process::Command;
use tracing::{debug, info, info_span, Instrument};

/// MinerU version this backend is pinned against.
///
/// Bumping is intentional — MinerU's output format has shifted across
/// major releases. A regression test against the eprint corpus gates
/// the bump.
pub const MINERU_VERSION: &str = "3.1.15";

/// Convert PDFs to Markdown by shelling out to MinerU via `uv run`.
pub struct LocalConverter {
    /// `uv` binary to invoke (defaults to `uv` on PATH).
    pub uv_binary: PathBuf,
    /// MinerU dependency spec.
    pub mineru_spec: String,
}

impl Default for LocalConverter {
    fn default() -> Self {
        Self {
            uv_binary: "uv".into(),
            mineru_spec: format!("mineru[core]=={MINERU_VERSION}"),
        }
    }
}

#[async_trait::async_trait]
impl Converter for LocalConverter {
    /// `LocalConverter` is ML-only. The `Text` tier is served by the
    /// downstream binary directly via the `pdf-extract` crate, so any
    /// `Quality` value is treated as ML.
    async fn convert(&self, pdf_path: &Path, _quality: Quality) -> Result<Conversion> {
        if !pdf_path.exists() {
            return Err(Error::InputNotFound(pdf_path.to_path_buf()));
        }
        self.convert_ml(pdf_path).await
    }
}

impl LocalConverter {
    async fn convert_ml(&self, pdf_path: &Path) -> Result<Conversion> {
        let start = Instant::now();
        let tmp = tempfile::tempdir()?;
        let span = info_span!("mineru_convert", paper = %pdf_path.display());
        async {
            info!(version = MINERU_VERSION, "starting MinerU");
            let output = Command::new(&self.uv_binary)
                .args(["run", "--quiet", "--with"])
                .arg(&self.mineru_spec)
                .args(["mineru", "-p"])
                .arg(pdf_path)
                .arg("-o")
                .arg(tmp.path())
                .args(["-b", "pipeline", "-m", "txt", "-t", "true", "-f", "true"])
                .env("MINERU_INTRA_OP_NUM_THREADS", "8")
                .env("MINERU_INTER_OP_NUM_THREADS", "2")
                .env("MINERU_DEVICE_MODE", "cpu")
                .output()
                .await?;
            if !output.status.success() {
                return Err(Error::Subprocess {
                    code: output.status.code(),
                    stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                });
            }
            let stem = pdf_path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| Error::Unexpected("pdf path has no stem".into()))?;
            let md_path = find_md(tmp.path(), stem).ok_or(Error::NoOutput)?;
            let markdown = tokio::fs::read_to_string(&md_path).await?;
            let duration = start.elapsed();
            debug!(?duration, bytes = markdown.len(), "MinerU completed");
            Ok(Conversion { markdown, quality: Quality::Ml, duration })
        }
        .instrument(span)
        .await
    }
}

fn find_md(root: &Path, stem: &str) -> Option<PathBuf> {
    let target_name = format!("{stem}.md");
    let mut stack = vec![root.to_path_buf()];
    while let Some(p) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&p) else { continue };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().is_some_and(|n| n == target_name.as_str()) {
                return Some(path);
            }
        }
    }
    None
}
