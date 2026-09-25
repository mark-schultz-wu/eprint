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
    crate::cache::models_dir(cache_root)
        .join(model)
        .join(&REVISION[..12])
}

/// Total size of the pinned weight files.
pub fn download_bytes() -> u64 {
    FILES.iter().map(|f| f.size).sum()
}

/// Ensure every weight file is present and verified; returns the model dir.
pub async fn ensure(cx: &Context) -> Result<PathBuf> {
    let dir = model_dir(&cx.cfg.cache_root);
    tokio::fs::create_dir_all(&dir).await?;
    prune_other_revisions(&dir);
    let fetch = Fetch {
        client: download_client(cx)?,
        base_url: format!("https://huggingface.co/{REPO}/resolve/{REVISION}"),
        offline: cx.offline,
        progress: !cx.json,
    };
    fetch.all(&dir, FILES).await?;
    Ok(dir)
}

/// Where and how to fetch weight files: `<base_url>/<file name>`.
struct Fetch {
    client: reqwest::Client,
    base_url: String,
    offline: bool,
    progress: bool,
}

impl Fetch {
    /// Download (or resume, or just verify) whichever of `files` aren't
    /// already verified in `dir`.
    async fn all(&self, dir: &Path, files: &[ModelFile]) -> Result<()> {
        let missing: Vec<&ModelFile> = files.iter().filter(|f| !is_verified(dir, f)).collect();
        if missing.is_empty() {
            return Ok(());
        }
        let total: u64 = missing.iter().map(|f| f.size).sum();
        if self.offline {
            bail!(
                "the Markdown converter's model weights ({:.1} GB) aren't downloaded yet and \
                 --offline forbids fetching them; re-run without --offline once to download \
                 them into {}",
                gb(total),
                dir.display(),
            );
        }
        if self.progress {
            eprintln!(
                "Downloading the Markdown model ({REPO}, {:.1} GB) into {} — one-time.",
                gb(total),
                dir.display(),
            );
        }
        for file in missing {
            let url = format!("{}/{}", self.base_url, file.name);
            download(&self.client, &url, dir, file, self.progress)
                .await
                .with_context(|| format!("downloading model file {}", file.name))?;
        }
        Ok(())
    }
}

/// Delete weights for revisions other than the pinned one (left behind when
/// an upgrade re-pins the model), so they don't silently hold gigabytes.
fn prune_other_revisions(current: &Path) {
    let (Some(parent), Some(keep)) = (current.parent(), current.file_name()) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name() != keep && entry.path().is_dir() {
            match std::fs::remove_dir_all(entry.path()) {
                Ok(()) => info!(dir = %entry.path().display(), "removed stale model revision"),
                Err(e) => {
                    tracing::warn!(dir = %entry.path().display(), error = %e, "could not remove stale model revision")
                }
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

async fn download(
    client: &reqwest::Client,
    url: &str,
    dir: &Path,
    f: &ModelFile,
    progress: bool,
) -> Result<()> {
    let dest = dir.join(f.name);
    let part = dir.join(format!("{}.part", f.name));

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
        let mut req = client.get(url);
        if have > 0 {
            req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
        }
        let resp = req.send().await.with_context(|| format!("GET {url}"))?;
        let resp = resp
            .error_for_status()
            .with_context(|| format!("GET {url}"))?;
        if resp.status() != reqwest::StatusCode::PARTIAL_CONTENT {
            // A full body: either we didn't ask for a range, or the server
            // ignored it. Either way, start from byte 0.
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
        let mut reporter = progress.then(|| Progress::new(f, have));
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.with_context(|| format!("reading {url}"))?;
            hasher.update(&chunk);
            out.write_all(&chunk).await?;
            have += chunk.len() as u64;
            if let Some(line) = reporter.as_mut().and_then(|r| r.update(have)) {
                eprintln!("{line}");
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

/// Progress lines for one large download: one per 5% of the file. Files
/// under 100 MB finish too fast to need any.
struct Progress {
    name: &'static str,
    size: u64,
    next: u64,
}

impl Progress {
    fn new(f: &ModelFile, have: u64) -> Self {
        let mut p = Self {
            name: f.name,
            size: f.size,
            next: 0,
        };
        p.next = p.after(have);
        p
    }

    fn after(&self, have: u64) -> u64 {
        have + self.size / 20
    }

    /// The line to print after reaching `have` bytes, if any.
    fn update(&mut self, have: u64) -> Option<String> {
        if self.size <= 100_000_000 || have < self.next {
            return None;
        }
        self.next = self.after(have);
        Some(format!(
            "  {}: {:.0}% ({:.2} / {:.2} GB)",
            self.name,
            100.0 * have as f64 / self.size as f64,
            gb(have),
            gb(self.size),
        ))
    }
}

/// Binary gigabytes, matching `eprint cache list`.
fn gb(bytes: u64) -> f64 {
    bytes as f64 / (1u64 << 30) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const CONFIG_BYTES: &[u8] = br#"{"fake": "config"}"#;
    const CONFIG: ModelFile = ModelFile {
        name: "config.json",
        size: 18,
        sha256: "1fd7af4283193e7e4a6d039b6ffb6413c315aa123563602047fa8d0976ab834a",
    };
    /// 100 KiB of `0..=255` repeated: big enough to resume partway.
    fn weights_bytes() -> Vec<u8> {
        (0..=255u8).cycle().take(102_400).collect()
    }
    const WEIGHTS: ModelFile = ModelFile {
        name: "model.safetensors",
        size: 102_400,
        sha256: "27783e87963a4efb6829b531c9ba57b44f45797f6770bd637fbf0d807cbdbae0",
    };

    fn fetch(server: &MockServer, offline: bool) -> (Fetch, String) {
        let base = format!("{}/repo", server.uri());
        let f = Fetch {
            client: reqwest::Client::new(),
            base_url: base.clone(),
            offline,
            progress: false,
        };
        (f, base)
    }

    async fn serve(server: &MockServer, name: &str, body: Vec<u8>) {
        Mock::given(method("GET"))
            .and(path(format!("/repo/{name}")))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
            .mount(server)
            .await;
    }

    async fn requests(server: &MockServer) -> usize {
        server.received_requests().await.unwrap().len()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn downloads_verifies_and_then_trusts_the_sidecars() {
        let server = MockServer::start().await;
        serve(&server, "config.json", CONFIG_BYTES.to_vec()).await;
        serve(&server, "model.safetensors", weights_bytes()).await;
        let dir = tempfile::tempdir().unwrap();
        let (f, _) = fetch(&server, false);

        f.all(dir.path(), &[CONFIG, WEIGHTS]).await.unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("config.json")).unwrap(),
            CONFIG_BYTES
        );
        assert_eq!(
            std::fs::read(dir.path().join("model.safetensors")).unwrap(),
            weights_bytes()
        );
        assert!(is_verified(dir.path(), &CONFIG) && is_verified(dir.path(), &WEIGHTS));
        assert!(!dir.path().join("model.safetensors.part").exists());
        assert_eq!(requests(&server).await, 2);
        let reqs = server.received_requests().await.unwrap();
        assert!(
            reqs.iter().all(|r| !r.headers.contains_key("range")),
            "a fresh download doesn't ask for a range"
        );

        f.all(dir.path(), &[CONFIG, WEIGHTS]).await.unwrap();
        assert_eq!(
            requests(&server).await,
            2,
            "verified files aren't re-fetched"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn resumes_a_partial_download_with_a_range_request() {
        let server = MockServer::start().await;
        let full = weights_bytes();
        Mock::given(method("GET"))
            .and(path("/repo/model.safetensors"))
            .and(header("range", "bytes=40000-"))
            .respond_with(ResponseTemplate::new(206).set_body_bytes(full[40_000..].to_vec()))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("model.safetensors.part"), &full[..40_000]).unwrap();
        let (f, _) = fetch(&server, false);

        f.all(dir.path(), &[WEIGHTS]).await.unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("model.safetensors")).unwrap(),
            full
        );
        assert!(is_verified(dir.path(), &WEIGHTS));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn restarts_when_the_server_ignores_the_range() {
        let server = MockServer::start().await;
        serve(&server, "model.safetensors", weights_bytes()).await; // always 200, full body
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("model.safetensors.part"),
            &weights_bytes()[..40_000],
        )
        .unwrap();
        let (f, _) = fetch(&server, false);

        f.all(dir.path(), &[WEIGHTS]).await.unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("model.safetensors")).unwrap(),
            weights_bytes()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_oversized_part_is_discarded() {
        let server = MockServer::start().await;
        serve(&server, "config.json", CONFIG_BYTES.to_vec()).await;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.json.part"), vec![b'x'; 100]).unwrap();
        let (f, _) = fetch(&server, false);

        f.all(dir.path(), &[CONFIG]).await.unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("config.json")).unwrap(),
            CONFIG_BYTES
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_corrupt_download_is_rejected_and_deleted() {
        let server = MockServer::start().await;
        serve(&server, "config.json", br#"{"fake": "CONFIG"}"#.to_vec()).await; // same size, wrong bytes
        let dir = tempfile::tempdir().unwrap();
        let (f, _) = fetch(&server, false);

        let err = format!("{:#}", f.all(dir.path(), &[CONFIG]).await.unwrap_err());
        assert!(err.contains("failed verification"), "{err}");
        assert!(!dir.path().join("config.json").exists());
        assert!(
            !dir.path().join("config.json.part").exists(),
            "a retry starts fresh"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_http_error_is_reported() {
        let server = MockServer::start().await; // nothing mounted: 404
        let dir = tempfile::tempdir().unwrap();
        let (f, base) = fetch(&server, false);
        let err = format!("{:#}", f.all(dir.path(), &[CONFIG]).await.unwrap_err());
        assert!(err.contains(&format!("{base}/config.json")), "{err}");
        assert!(err.contains("downloading model file config.json"), "{err}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_complete_part_is_verified_without_downloading() {
        let server = MockServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("model.safetensors.part"), weights_bytes()).unwrap();
        let (f, _) = fetch(&server, false);

        f.all(dir.path(), &[WEIGHTS]).await.unwrap();
        assert!(is_verified(dir.path(), &WEIGHTS));
        assert_eq!(requests(&server).await, 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn offline_refuses_to_download_but_accepts_verified_files() {
        let server = MockServer::start().await;
        serve(&server, "config.json", CONFIG_BYTES.to_vec()).await;
        let dir = tempfile::tempdir().unwrap();

        let (offline, _) = fetch(&server, true);
        let err = offline
            .all(dir.path(), &[CONFIG])
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("--offline forbids fetching them"), "{err}");
        assert_eq!(requests(&server).await, 0);

        let (online, _) = fetch(&server, false);
        online.all(dir.path(), &[CONFIG]).await.unwrap();
        offline.all(dir.path(), &[CONFIG]).await.unwrap();
    }

    #[test]
    fn progress_reports_every_five_percent_of_large_files_only() {
        let big = ModelFile {
            name: "model.safetensors",
            size: 2_000_000_000,
            sha256: "",
        };
        let mut p = Progress::new(&big, 0);
        assert_eq!(p.update(99_999_999), None);
        assert_eq!(
            p.update(100_000_000).as_deref(),
            Some("  model.safetensors: 5% (0.09 / 1.86 GB)")
        );
        assert_eq!(p.update(150_000_000), None, "next line at 10%");
        assert!(p.update(200_000_000).unwrap().contains(": 10% "));

        // Resuming at 50%: the first line comes at 55%.
        let mut resumed = Progress::new(&big, 1_000_000_000);
        assert_eq!(resumed.update(1_050_000_000), None);
        assert!(resumed.update(1_100_000_000).unwrap().contains(": 55% "));

        let small = ModelFile {
            name: "tokenizer.json",
            size: 100_000_000,
            sha256: "",
        };
        assert_eq!(Progress::new(&small, 0).update(100_000_000), None);
    }

    #[test]
    fn gb_is_binary_gigabytes() {
        assert_eq!(gb(1 << 30), 1.0);
        assert_eq!(gb(3 << 29), 1.5);
        assert_eq!(gb(0), 0.0);
    }

    #[test]
    fn download_size_is_the_sum_of_the_pinned_files() {
        assert_eq!(download_bytes(), 2_323_553_561);
    }

    #[test]
    fn model_dir_is_keyed_by_revision() {
        let d = model_dir(Path::new("/c"));
        assert_eq!(
            d,
            Path::new("/c/models/MinerU2.5-Pro-2605-1.2B/bff20d4ae2bf")
        );
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
