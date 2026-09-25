//! Fetch and verify the MinerU2.5-Pro weights.
//!
//! The model is pinned to one Hugging Face commit, and every file it loads is
//! pinned by size + SHA-256. Files download on first use into
//! `<cache_root>/models/<model>/<revision>/`, resuming a partial `.part` file
//! if a previous download was interrupted. A verified file gets a `.sha256`
//! sidecar so later runs don't re-hash 2.2 GB.

use crate::cli::Context;
use anyhow::{bail, Context as _, Result};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::info;

pub const REPO: &str = "opendatalab/MinerU2.5-Pro-2605-1.2B";
pub const REVISION: &str = "bff20d4ae2bf202df9f45284b4d43681555a97ed";

struct ModelFile {
    name: &'static str,
    size: u64,
    sha256: &'static str,
}

/// Exactly the files `oar_ocr_vl::MinerU::from_dir` reads.
const FILES: &[ModelFile] = &[
    ModelFile {
        name: "config.json",
        size: 2840,
        sha256: "22097df08750242647a513043636a8dff16820a09757e9271e220bdea378df28",
    },
    ModelFile {
        name: "preprocessor_config.json",
        size: 316,
        sha256: "7070ae84a684ce2eb8d239c2cb38ff848085075784b213ea28a5ef5b3cdb445f",
    },
    ModelFile {
        name: "generation_config.json",
        size: 215,
        sha256: "405a603af8aed51b82ef71a5e16e7c053d11576fdc2db3c168d32bac2fd75f0d",
    },
    ModelFile {
        name: "tokenizer.json",
        size: 11_423_550,
        sha256: "dceac5fc54a795ee7570d17902b47bd05412dc2afa62bdf325c3f97fcb5b87fe",
    },
    ModelFile {
        name: "model.safetensors",
        size: 2_312_126_640,
        sha256: "abf8681ca63b8dec7b67de257af47b821f179442f72998d0696ae2ed9232a5f0",
    },
];

/// Where the pinned weights live (whether or not they're downloaded yet).
pub fn model_dir(cache_root: &Path) -> PathBuf {
    let model = REPO.rsplit('/').next().unwrap_or(REPO);
    cache_root.join("models").join(model).join(&REVISION[..12])
}

/// Ensure every weight file is present and verified; returns the model dir.
pub async fn ensure(cx: &Context) -> Result<PathBuf> {
    let dir = model_dir(&cx.cfg.cache_root);
    tokio::fs::create_dir_all(&dir).await?;
    prune_other_revisions(&dir);
    let missing: Vec<&ModelFile> = FILES.iter().filter(|f| !is_verified(&dir, f)).collect();
    if missing.is_empty() {
        return Ok(dir);
    }
    let total: u64 = missing.iter().map(|f| f.size).sum();
    if cx.offline {
        bail!(
            "the Markdown converter's model weights ({:.1} GB) aren't downloaded yet and \
             --offline forbids fetching them; re-run without --offline once to download \
             them into {}",
            gb(total),
            dir.display(),
        );
    }
    if !cx.json {
        eprintln!(
            "Downloading the Markdown model ({REPO}, {:.1} GB) into {} — one-time.",
            gb(total),
            dir.display(),
        );
    }
    let client = download_client(cx)?;
    for file in missing {
        download(&client, &dir, file, !cx.json)
            .await
            .with_context(|| format!("downloading model file {}", file.name))?;
    }
    Ok(dir)
}

/// Delete weights for revisions other than the pinned one (left behind when
/// an upgrade re-pins the model), so they don't silently hold gigabytes.
fn prune_other_revisions(current: &Path) {
    let (Some(parent), Some(keep)) = (current.parent(), current.file_name()) else { return };
    let Ok(entries) = std::fs::read_dir(parent) else { return };
    for entry in entries.flatten() {
        if entry.file_name() != keep && entry.path().is_dir() {
            match std::fs::remove_dir_all(entry.path()) {
                Ok(()) => info!(dir = %entry.path().display(), "removed stale model revision"),
                Err(e) => tracing::warn!(dir = %entry.path().display(), error = %e, "could not remove stale model revision"),
            }
        }
    }
}

fn is_verified(dir: &Path, f: &ModelFile) -> bool {
    let path = dir.join(f.name);
    let size_ok = std::fs::metadata(&path).is_ok_and(|m| m.len() == f.size);
    size_ok && std::fs::read_to_string(sidecar(&path)).is_ok_and(|s| s.trim() == f.sha256)
}

fn sidecar(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".sha256");
    PathBuf::from(s)
}

/// A client for multi-GB downloads: unlike `http::client` there's no total
/// timeout, only connect/read stall timeouts. Hugging Face isn't eprint, so
/// the eprint rate limiter doesn't apply.
fn download_client(cx: &Context) -> Result<reqwest::Client> {
    let ua = crate::iacr::http::user_agent(cx.cfg.network.contact.as_deref());
    reqwest::Client::builder()
        .user_agent(ua)
        .connect_timeout(Duration::from_secs(30))
        .read_timeout(Duration::from_secs(60))
        .build()
        .context("building download client")
}

async fn download(client: &reqwest::Client, dir: &Path, f: &ModelFile, progress: bool) -> Result<()> {
    let dest = dir.join(f.name);
    let part = dir.join(format!("{}.part", f.name));
    let url = format!("https://huggingface.co/{REPO}/resolve/{REVISION}/{}", f.name);

    // Resume: hash whatever an earlier attempt already wrote, then ask for the rest.
    let mut hasher = Sha256::new();
    let mut have = 0u64;
    if let Ok(mut existing) = tokio::fs::File::open(&part).await {
        if existing.metadata().await?.len() <= f.size {
            let mut buf = vec![0u8; 8 << 20];
            loop {
                let n = existing.read(&mut buf).await?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
                have += n as u64;
            }
        }
    }
    // A complete `.part` (e.g. a crash between download and rename) only needs verifying.
    if have < f.size {
        let mut req = client.get(&url);
        if have > 0 {
            req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
        }
        let resp = req.send().await.with_context(|| format!("GET {url}"))?;
        let resp = resp.error_for_status().with_context(|| format!("GET {url}"))?;
        if have > 0 && resp.status() != reqwest::StatusCode::PARTIAL_CONTENT {
            // Server ignored the range; start over.
            hasher = Sha256::new();
            have = 0;
        }
        let mut out = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(have > 0)
            .truncate(have == 0)
            .open(&part)
            .await?;

        let mut stream = resp.bytes_stream();
        let mut next_report = have + f.size / 20;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.with_context(|| format!("reading {url}"))?;
            hasher.update(&chunk);
            out.write_all(&chunk).await?;
            have += chunk.len() as u64;
            if progress && f.size > 100_000_000 && have >= next_report {
                eprintln!(
                    "  {}: {:.0}% ({:.2} / {:.2} GB)",
                    f.name,
                    100.0 * have as f64 / f.size as f64,
                    gb(have),
                    gb(f.size),
                );
                next_report = have + f.size / 20;
            }
        }
        out.flush().await?;
    }

    let digest = format!("{:x}", hasher.finalize());
    if have != f.size || digest != f.sha256 {
        let _ = tokio::fs::remove_file(&part).await;
        bail!(
            "{} failed verification (got {have} bytes, sha256 {digest}; expected {} bytes, \
             sha256 {}); the partial file was deleted, so re-running starts fresh",
            f.name,
            f.size,
            f.sha256,
        );
    }
    tokio::fs::rename(&part, &dest).await?;
    tokio::fs::write(sidecar(&dest), f.sha256).await?;
    info!(file = f.name, bytes = f.size, "model file verified");
    Ok(())
}

/// Binary gigabytes, matching `eprint cache list`.
fn gb(bytes: u64) -> f64 {
    bytes as f64 / (1u64 << 30) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_dir_is_keyed_by_revision() {
        let d = model_dir(Path::new("/c"));
        assert_eq!(d, Path::new("/c/models/MinerU2.5-Pro-2605-1.2B/bff20d4ae2bf"));
    }

    #[test]
    fn prunes_only_other_revisions() {
        let root = tempfile::tempdir().unwrap();
        let current = model_dir(root.path());
        let stale = current.with_file_name("0123456789ab");
        std::fs::create_dir_all(&current).unwrap();
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::write(current.join("config.json"), b"{}").unwrap();
        prune_other_revisions(&current);
        assert!(!stale.exists());
        assert!(current.join("config.json").exists());
    }

    #[test]
    fn verification_needs_matching_size_and_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let f = &FILES[0];
        let path = dir.path().join(f.name);
        assert!(!is_verified(dir.path(), f));
        std::fs::write(&path, vec![b'x'; f.size as usize]).unwrap();
        assert!(!is_verified(dir.path(), f), "no sidecar yet");
        std::fs::write(sidecar(&path), f.sha256).unwrap();
        assert!(is_verified(dir.path(), f));
        std::fs::write(&path, b"short").unwrap();
        assert!(!is_verified(dir.path(), f), "size mismatch");
    }
}
