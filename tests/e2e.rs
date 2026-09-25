//! End-to-end tests: run the real `eprint` binary against a local fake
//! eprint server (via `EPRINT_BASE_URL`) with a throwaway cache, and check
//! exit codes, output, and exactly what lands on disk.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ID: &str = "2024/463";
const V1: &str = "20240319T143540Z";
const V1_UNIX: u64 = 1_710_858_940;
const V2: &str = "20250106T174348Z";
const V2_UNIX: u64 = 1_736_185_428;
const PDF_V1: &[u8] = b"%PDF-1.4 fake paper, version 1";
const PDF_V2: &[u8] = b"%PDF-1.4 fake paper, version 2";
const TITLE: &str = "Security Guidelines for Implementing Homomorphic Encryption";

/// A fake eprint plus an empty cache.
struct Harness {
    server: MockServer,
    cache: TempDir,
}

impl Harness {
    async fn new() -> Self {
        Self {
            server: MockServer::start().await,
            cache: tempfile::tempdir().unwrap(),
        }
    }

    /// Run `eprint <args>` against the fake server. Auto-sync is off unless
    /// `env` turns it on; rate limiting is effectively off.
    async fn run(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_eprint"));
        cmd.args(args)
            .env("EPRINT_BASE_URL", self.server.uri())
            .env("EPRINT_CACHE_DIR", self.cache.path())
            .env("EPRINT_AUTO_SYNC", "false")
            .env("EPRINT_MIN_INTERVAL_S", "0.001")
            .env("NO_COLOR", "1");
        for var in [
            "RUST_LOG",
            "HTTP_PROXY",
            "http_proxy",
            "ALL_PROXY",
            "all_proxy",
        ] {
            cmd.env_remove(var);
        }
        for (k, v) in env {
            cmd.env(k, v);
        }
        tokio::task::spawn_blocking(move || cmd.output().unwrap())
            .await
            .unwrap()
    }

    fn version_dir(&self, version: &str) -> PathBuf {
        self.cache.path().join("2024/0463").join(version)
    }

    async fn serve(&self, url_path: &str, body: impl Into<Vec<u8>>) {
        Mock::given(method("GET"))
            .and(path(url_path))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body.into()))
            .mount(&self.server)
            .await;
    }

    /// A paper whose archive lists `versions` (oldest first; the last is
    /// current), with a landing page and a PDF per version.
    async fn serve_paper(&self, versions: &[(&str, u64, &[u8])]) {
        self.serve("/archive/versions/2024/463", archive_page(versions))
            .await;
        self.serve("/2024/463", LANDING).await;
        for (_, unix, pdf) in versions {
            self.serve(&format!("/archive/2024/463/{unix}.pdf"), *pdf)
                .await;
        }
    }

    async fn requests_to(&self, url_path: &str) -> usize {
        let received = self.server.received_requests().await.unwrap_or_default();
        received.iter().filter(|r| r.url.path() == url_path).count()
    }
}

fn archive_page(versions: &[(&str, u64, &[u8])]) -> String {
    let mut items: Vec<String> = versions
        .iter()
        .map(|(v, _, _)| {
            // Archive pages use the compact `YYYYMMDD:HHMMSS` form.
            let compact = format!("{}:{}", &v[..8], &v[9..15]);
            format!(r#"<li><a href="/archive/2024/463/{compact}">{compact}</a> PDF update</li>"#)
        })
        .collect();
    if let Some(last) = items.last_mut() {
        *last = last.replace("PDF update", "PDF update (most recent)");
    }
    items.reverse(); // newest first, like the real page
    format!(
        "<html><body><h2>Versions for ePrint paper 2024/463</h2><ul>{}</ul></body></html>",
        items.join("\n")
    )
}

const LANDING: &str = r#"<html><body>
<h3 class="mb-3">Security Guidelines for Implementing Homomorphic Encryption</h3>
<div class="author"><span class="authorName">Anyone</span></div>
<h5 class="mt-3">Abstract</h5>
<p>Fully Homomorphic Encryption is a cryptographic primitive.</p>
<dt>Category</dt><dd>Public-key cryptography</dd>
<pre id="bibtex">@misc{cryptoeprint:2024/463, title = {Security Guidelines}}</pre>
</body></html>"#;

fn oai_list_records(id: &str, datestamp: &str) -> String {
    format!(
        r#"<?xml version="1.0"?><OAI-PMH><ListRecords><record><header>
        <identifier>oai:eprint.iacr.org:{id}</identifier><datestamp>{datestamp}</datestamp>
        </header></record></ListRecords></OAI-PMH>"#
    )
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn json(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("bad JSON ({e}):\n{}\n{}", stdout(out), stderr(out)))
}

fn read(p: &Path) -> Vec<u8> {
    std::fs::read(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

#[tokio::test(flavor = "multi_thread")]
async fn fresh_fetch_files_the_current_pdf_and_metadata() {
    let h = Harness::new().await;
    h.serve_paper(&[(V1, V1_UNIX, PDF_V1), (V2, V2_UNIX, PDF_V2)])
        .await;

    let out = h.run(&["--json", "paper", ID], &[]).await;
    assert!(out.status.success(), "{}", stderr(&out));
    let report = json(&out);
    assert_eq!(report["title"], TITLE);
    assert_eq!(report["current_version"], V2);
    assert_eq!(report["resolved_version"], V2);
    assert_eq!(report["known_versions"], serde_json::json!([V1, V2]));

    let dir = h.version_dir(V2);
    assert_eq!(read(&dir.join("paper.pdf")), PDF_V2);
    assert!(String::from_utf8(read(&dir.join("paper.bib")))
        .unwrap()
        .starts_with("@misc{cryptoeprint:2024/463"));
    assert!(dir.join("abstract.txt").exists());
    // Fetched by exact version, never via the "latest PDF" URL.
    assert_eq!(h.requests_to("/2024/463.pdf").await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn historical_version_is_fetched_by_timestamp() {
    let h = Harness::new().await;
    h.serve_paper(&[(V1, V1_UNIX, PDF_V1), (V2, V2_UNIX, PDF_V2)])
        .await;

    let out = h.run(&["--json", "paper", ID, "--version", V1], &[]).await;
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(json(&out)["resolved_version"], V1);
    assert_eq!(read(&h.version_dir(V1).join("paper.pdf")), PDF_V1);
    // The landing page describes the current version only.
    assert!(!h.version_dir(V1).join("abstract.txt").exists());
    assert!(!h.version_dir(V2).exists());
}

/// Regression: after a revision, `sync` + `paper` must pick up the new
/// version and file each PDF under its own version. (The bug: the old
/// version stayed "current" and the newest PDF was saved under it.)
#[tokio::test(flavor = "multi_thread")]
async fn a_revision_seen_by_sync_is_fetched_into_its_own_directory() {
    let h = Harness::new().await;
    h.serve_paper(&[(V1, V1_UNIX, PDF_V1)]).await;
    let out = h.run(&["paper", ID], &[]).await;
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(read(&h.version_dir(V1).join("paper.pdf")), PDF_V1);

    // The paper is revised: the archive gains V2 and OAI reports the change.
    h.server.reset().await;
    h.serve_paper(&[(V1, V1_UNIX, PDF_V1), (V2, V2_UNIX, PDF_V2)])
        .await;
    Mock::given(method("GET"))
        .and(path("/oai"))
        .and(query_param("verb", "ListRecords"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(oai_list_records(ID, "2025-01-06T17:43:48Z")),
        )
        .mount(&h.server)
        .await;

    let out = h.run(&["sync"], &[]).await;
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("1 cached papers updated"),
        "{}",
        stdout(&out)
    );

    let out = h.run(&["--json", "paper", ID], &[]).await;
    assert!(out.status.success(), "{}", stderr(&out));
    let report = json(&out);
    assert_eq!(report["current_version"], V2);
    assert_eq!(report["resolved_version"], V2);
    assert_eq!(read(&h.version_dir(V2).join("paper.pdf")), PDF_V2);
    assert_eq!(read(&h.version_dir(V1).join("paper.pdf")), PDF_V1);
}

#[tokio::test(flavor = "multi_thread")]
async fn offline_serves_the_cache_without_touching_the_network() {
    let h = Harness::new().await;
    h.serve_paper(&[(V1, V1_UNIX, PDF_V1)]).await;
    assert!(h.run(&["paper", ID], &[]).await.status.success());

    h.server.reset().await;
    let out = h.run(&["--offline", "paper", ID], &[]).await;
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains(TITLE));
    assert!(h.server.received_requests().await.unwrap().is_empty());

    // An uncached paper can't be resolved offline: exit code 2.
    let out = h.run(&["--offline", "paper", "2023/805"], &[]).await;
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_paper_exits_2_and_says_the_id_is_probably_wrong() {
    let h = Harness::new().await;
    h.serve(
        "/archive/versions/2024/999999",
        "<html><body>no such paper</body></html>",
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/oai"))
        .and(query_param("verb", "GetRecord"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"<OAI-PMH><error code="idDoesNotExist">no such id</error></OAI-PMH>"#,
        ))
        .mount(&h.server)
        .await;

    let out = h.run(&["paper", "2024/999999"], &[]).await;
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("usually means the id is wrong"),
        "{}",
        stderr(&out)
    );
}

/// Regression: a failing auto-sync used to abort every `paper` command.
#[tokio::test(flavor = "multi_thread")]
async fn a_failing_auto_sync_does_not_block_a_cached_paper() {
    let h = Harness::new().await;
    h.serve_paper(&[(V1, V1_UNIX, PDF_V1)]).await;
    assert!(h.run(&["paper", ID], &[]).await.status.success());

    Mock::given(method("GET"))
        .and(path("/oai"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&h.server)
        .await;
    let out = h.run(&["paper", ID], &[("EPRINT_AUTO_SYNC", "true")]).await;
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains(TITLE));
    assert!(
        stderr(&out).contains("auto-sync failed"),
        "{}",
        stderr(&out)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_non_pdf_response_is_rejected_and_not_cached() {
    let h = Harness::new().await;
    h.serve(
        "/archive/versions/2024/463",
        archive_page(&[(V1, V1_UNIX, PDF_V1)]),
    )
    .await;
    h.serve(
        &format!("/archive/2024/463/{V1_UNIX}.pdf"),
        "<html>Not a PDF</html>",
    )
    .await;

    let out = h.run(&["paper", ID], &[]).await;
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("don't look like a PDF"),
        "{}",
        stderr(&out)
    );
    assert!(!h.version_dir(V1).join("paper.pdf").exists());
}

/// Regression: logs went to stdout, so any warning corrupted `--json` output.
#[tokio::test(flavor = "multi_thread")]
async fn warnings_never_corrupt_json_output() {
    let h = Harness::new().await;
    // An invalid env value triggers a warning on every command.
    let out = h
        .run(
            &["--json", "cache", "list"],
            &[("EPRINT_AUTO_SYNC", "maybe")],
        )
        .await;
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(json(&out), serde_json::json!([]));
    assert!(
        stderr(&out).contains("ignoring EPRINT_AUTO_SYNC"),
        "{}",
        stderr(&out)
    );
}
