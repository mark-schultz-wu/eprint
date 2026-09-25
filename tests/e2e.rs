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

    let out = h.run(&["--json", "paper", ID, "--at", V1], &[]).await;
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
    // Auto-sync is enabled and due (never synced), but --offline wins.
    let out = h
        .run(&["--offline", "paper", ID], &[("EPRINT_AUTO_SYNC", "true")])
        .await;
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains(TITLE));
    assert!(!stderr(&out).contains("auto-sync"), "{}", stderr(&out));
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

// ---------------------------------------------------------------------------
// Coverage for paths cargo-mutants showed no test exercised.
// ---------------------------------------------------------------------------

async fn serve_oai(h: &Harness, verb: &str, body: String) {
    Mock::given(method("GET"))
        .and(path("/oai"))
        .and(query_param("verb", verb))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&h.server)
        .await;
}

/// JSON report details: actions, bytes, cached versions, and the version
/// meta written for the fetch.
#[tokio::test(flavor = "multi_thread")]
async fn report_counts_actions_bytes_and_cached_versions() {
    let h = Harness::new().await;
    h.serve_paper(&[(V1, V1_UNIX, PDF_V1), (V2, V2_UNIX, PDF_V2)])
        .await;

    let report = json(&h.run(&["--json", "paper", ID], &[]).await);
    assert_eq!(
        report["actions"],
        serde_json::json!(["archive-listed", "fetched-pdf"])
    );
    let landing_bytes = LANDING.len() as u64;
    assert_eq!(
        report["bytes_downloaded"],
        PDF_V2.len() as u64 + landing_bytes
    );
    assert_eq!(report["cached_versions"], serde_json::json!([V2]));

    let meta: Value = serde_json::from_slice(&read(&h.version_dir(V2).join("meta.json"))).unwrap();
    let fetched = meta["fetched_unix_s"]
        .as_i64()
        .expect("fetch time recorded");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    assert!(
        (now - fetched).abs() < 300,
        "fetched_unix_s={fetched}, now={now}"
    );

    // A second historical fetch: the title is already known, so the landing
    // page isn't requested again, and both versions now show as cached.
    let report = json(&h.run(&["--json", "paper", ID, "--at", V1], &[]).await);
    assert_eq!(
        report["actions"],
        serde_json::json!(["fetched-historical-pdf"])
    );
    assert_eq!(report["bytes_downloaded"], PDF_V1.len() as u64);
    assert_eq!(report["cached_versions"], serde_json::json!([V1, V2]));
    assert_eq!(h.requests_to("/2024/463").await, 1);
}

/// The human-readable report.
#[tokio::test(flavor = "multi_thread")]
async fn human_output_lists_versions_actions_and_abstract() {
    let h = Harness::new().await;
    h.serve_paper(&[(V1, V1_UNIX, PDF_V1), (V2, V2_UNIX, PDF_V2)])
        .await;

    // The bare `eprint <id>` shorthand.
    let out = stdout(&h.run(&[ID, "--at", V1], &[]).await);
    assert!(out.starts_with("2024/463\n"), "{out}");
    assert!(
        out.contains(&format!("  title:            {TITLE}\n")),
        "{out}"
    );
    assert!(
        out.contains(&format!("  current version:  {V2}\n")),
        "{out}"
    );
    assert!(
        out.contains(&format!("  resolved to:      {V1}\n")),
        "{out}"
    );
    assert!(
        out.contains("  versions:         2 known, 1 cached\n"),
        "{out}"
    );
    assert!(out.contains(&format!("\n    {V2}  (current)\n")), "{out}");
    assert!(out.contains(&format!("\n    {V1}  (cached)\n")), "{out}");
    assert!(
        out.contains("  did:              archive-listed, fetched-historical-pdf\n"),
        "{out}"
    );
    let downloaded = format!("  downloaded:       {} B\n", PDF_V1.len() + LANDING.len());
    assert!(out.contains(&downloaded), "{out}");
    // The abstract belongs to the current version, which isn't cached yet.
    assert!(!out.contains("Abstract:"), "{out}");

    let out = stdout(&h.run(&["paper", ID], &[]).await);
    assert!(!out.contains("resolved to:"), "{out}");
    assert!(
        out.contains(&format!("\n    {V2}  (current, cached)\n")),
        "{out}"
    );
    assert!(
        out.contains("Abstract:\n  Fully Homomorphic Encryption"),
        "{out}"
    );
    let quiet = stdout(&h.run(&["paper", ID, "--no-abstract"], &[]).await);
    assert!(!quiet.contains("Abstract:"), "{quiet}");
    assert!(
        !quiet.contains("did:"),
        "nothing to do on a cache hit: {quiet}"
    );
    assert!(
        !quiet.contains("downloaded:"),
        "nothing downloaded on a cache hit: {quiet}"
    );
}

/// The archive listing is unreachable: OAI-PMH supplies the current
/// version, title, and abstract instead.
#[tokio::test(flavor = "multi_thread")]
async fn oai_fallback_resolves_a_paper_when_the_archive_is_down() {
    let h = Harness::new().await;
    Mock::given(method("GET"))
        .and(path("/archive/versions/2024/463"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&h.server)
        .await;
    serve_oai(
        &h,
        "GetRecord",
        r#"<OAI-PMH><GetRecord><record><header>
            <identifier>oai:eprint.iacr.org:2024/463</identifier>
            <datestamp>2025-01-06T17:43:48Z</datestamp></header>
          <metadata><dc><title>Title From OAI</title>
            <description>Abstract from OAI.</description></dc></metadata>
        </record></GetRecord></OAI-PMH>"#
            .into(),
    )
    .await;
    h.serve(&format!("/archive/2024/463/{V2_UNIX}.pdf"), PDF_V2)
        .await;
    // No landing page mock: that request 404s, so the abstract must come from OAI.

    let out = h.run(&["--json", "paper", ID], &[]).await;
    assert!(out.status.success(), "{}", stderr(&out));
    let report = json(&out);
    assert_eq!(report["resolved_version"], V2);
    assert_eq!(report["title"], "Title From OAI");
    assert_eq!(
        report["known_versions"],
        serde_json::json!([V2]),
        "no duplicates"
    );
    assert_eq!(
        report["actions"],
        serde_json::json!(["oai-resolved", "fetched-pdf"])
    );
    assert_eq!(read(&h.version_dir(V2).join("paper.pdf")), PDF_V2);
    assert_eq!(
        read(&h.version_dir(V2).join("abstract.txt")),
        b"Abstract from OAI."
    );
}

/// No source has the PDF: exit code 3, naming the URL that failed.
#[tokio::test(flavor = "multi_thread")]
async fn a_missing_pdf_exits_3_and_names_the_url() {
    let h = Harness::new().await;
    h.serve(
        "/archive/versions/2024/463",
        archive_page(&[(V1, V1_UNIX, PDF_V1)]),
    )
    .await;
    let out = h.run(&["paper", ID], &[]).await;
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    let expected_url = format!("{}/archive/2024/463/{V1_UNIX}.pdf", h.server.uri());
    assert!(stderr(&out).contains(&expected_url), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("Sources tried: eprint-http: "),
        "{}",
        stderr(&out)
    );
}

/// Offline, an uncached version of a cached paper fails without any
/// network request (network sources are skipped), exit code 3.
#[tokio::test(flavor = "multi_thread")]
async fn offline_skips_network_sources() {
    let h = Harness::new().await;
    h.serve_paper(&[(V1, V1_UNIX, PDF_V1), (V2, V2_UNIX, PDF_V2)])
        .await;
    assert!(h.run(&["paper", ID], &[]).await.status.success());
    h.server.reset().await;

    let out = h.run(&["--offline", "paper", ID, "--at", V1], &[]).await;
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("--offline skips network sources"),
        "{}",
        stderr(&out)
    );
    assert!(h.server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn requests_carry_the_eprint_user_agent() {
    let h = Harness::new().await;
    h.serve_paper(&[(V1, V1_UNIX, PDF_V1)]).await;
    let out = h
        .run(&["paper", ID], &[("EPRINT_CONTACT", "me@example.com")])
        .await;
    assert!(out.status.success(), "{}", stderr(&out));
    for req in h.server.received_requests().await.unwrap() {
        let ua = req.headers.get("user-agent").unwrap().to_str().unwrap();
        assert!(ua.starts_with("eprint/"), "{ua}");
        assert!(ua.ends_with(" me@example.com"), "{ua}");
    }
}

/// `sync` follows OAI resumption tokens: the cached paper's record is on
/// the second page.
#[tokio::test(flavor = "multi_thread")]
async fn sync_follows_resumption_tokens() {
    let h = Harness::new().await;
    h.serve_paper(&[(V1, V1_UNIX, PDF_V1)]).await;
    assert!(h.run(&["paper", ID], &[]).await.status.success());

    Mock::given(method("GET"))
        .and(path("/oai"))
        .and(query_param("resumptionToken", "page-2"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(oai_list_records(ID, "2025-01-06T17:43:48Z")),
        )
        .with_priority(1)
        .mount(&h.server)
        .await;
    serve_oai(
        &h,
        "ListRecords",
        r#"<OAI-PMH><ListRecords><record><header>
            <identifier>oai:eprint.iacr.org:2026/001</identifier>
            <datestamp>2026-01-01T00:00:00Z</datestamp></header></record>
          <resumptionToken>page-2</resumptionToken></ListRecords></OAI-PMH>"#
            .into(),
    )
    .await;

    let out = h
        .run(&["--json", "sync", "--since", "2026-01-01"], &[])
        .await;
    assert!(out.status.success(), "{}", stderr(&out));
    let report = json(&out);
    assert_eq!(report["from"], "2026-01-01");
    assert_eq!(report["records_seen"], 2);
    assert_eq!(report["cached_papers_updated"], 1);
}

/// Auto-sync runs only when the last sync is older than the threshold, and
/// then asks OAI for records since that day.
#[tokio::test(flavor = "multi_thread")]
async fn auto_sync_runs_only_when_stale() {
    let h = Harness::new().await;
    h.serve_paper(&[(V1, V1_UNIX, PDF_V1)]).await;
    serve_oai(
        &h,
        "ListRecords",
        oai_list_records("2026/001", "2026-01-01T00:00:00Z"),
    )
    .await;
    assert!(h.run(&["paper", ID], &[]).await.status.success());

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    // The stamp file is the cache's record of the last sync (unix seconds).
    let stamp = h.cache.path().join(".last_sync_unix_s");
    let auto = [("EPRINT_AUTO_SYNC", "true")];

    std::fs::write(&stamp, (now - 3600).to_string()).unwrap();
    let out = h.run(&["paper", ID], &auto).await;
    assert!(
        !stderr(&out).contains("auto-syncing"),
        "fresh: {}",
        stderr(&out)
    );
    assert_eq!(h.requests_to("/oai").await, 0);

    let last = now - 25 * 3600;
    std::fs::write(&stamp, last.to_string()).unwrap();
    let out = h.run(&["paper", ID], &auto).await;
    assert!(
        stderr(&out).contains("last sync: 25h ago"),
        "{}",
        stderr(&out)
    );
    let reqs = h.server.received_requests().await.unwrap();
    let oai: Vec<_> = reqs.iter().filter(|r| r.url.path() == "/oai").collect();
    assert_eq!(oai.len(), 1);
    let from = oai[0]
        .url
        .query_pairs()
        .find(|(k, _)| k == "from")
        .map(|(_, v)| v.into_owned());
    let day = time::OffsetDateTime::from_unix_timestamp(last as i64)
        .unwrap()
        .date()
        .to_string();
    assert_eq!(from.as_deref(), Some(day.as_str()));
    // Under --json, auto-sync still runs but prints nothing.
    std::fs::write(&stamp, last.to_string()).unwrap();
    let out = h.run(&["--json", "paper", ID], &auto).await;
    assert!(out.status.success(), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(
        !err.contains("auto-sync") && !err.contains("done ("),
        "{err}"
    );
    assert_eq!(h.requests_to("/oai").await, 2);
    // The run advanced the stamp.
    let written: u64 = std::fs::read_to_string(&stamp)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(written >= now, "stamp advanced");
}

/// `cache clear`: dry run, clear (keeping the model and foreign dirs), and
/// `--models`.
#[tokio::test(flavor = "multi_thread")]
async fn cache_clear_deletes_papers_keeps_foreign_dirs_and_the_model() {
    let h = Harness::new().await;
    h.serve_paper(&[(V1, V1_UNIX, PDF_V1)]).await;
    assert!(h.run(&["paper", ID], &[]).await.status.success());
    let root = h.cache.path();
    std::fs::create_dir_all(root.join("2019/0999")).unwrap(); // not ours
    std::fs::create_dir_all(root.join("models/some-model")).unwrap();
    std::fs::write(root.join("models/some-model/w.bin"), [0u8; 2048]).unwrap();

    let out = stdout(&h.run(&["cache", "clear", "--dry-run"], &[]).await);
    assert!(out.contains("would delete 1 paper,"), "{out}");
    assert!(
        out.contains("(1 numbered directory without an eprint meta.json would be left in place)"),
        "{out}"
    );
    assert!(
        out.contains("would keep the Markdown model (2.0 KB)"),
        "{out}"
    );
    assert!(
        h.version_dir(V1).join("paper.pdf").exists(),
        "dry run deletes nothing"
    );

    let out = stdout(&h.run(&["cache", "clear"], &[]).await);
    assert!(out.contains("deleted 1 paper,"), "{out}");
    assert!(out.contains("was left in place"), "{out}");
    assert!(out.contains("kept the Markdown model (2.0 KB)"), "{out}");
    assert!(!root.join("2024").exists());
    assert!(root.join("2019/0999").exists());
    assert!(root.join("models/some-model/w.bin").exists());

    let out = stdout(&h.run(&["cache", "clear", "--models"], &[]).await);
    assert!(out.contains("deleted 0 papers,"), "{out}");
    assert!(out.contains("deleted the Markdown model, 2.0 KB"), "{out}");
    assert!(!root.join("models").exists());
    assert!(root.join("2019/0999").exists());

    let out = stdout(&h.run(&["cache", "clear"], &[]).await);
    assert!(
        !out.contains("Markdown model"),
        "no model, no model line: {out}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn feed_filters_and_formats_items() {
    let h = Harness::new().await;
    let rss = r#"<rss><channel><item>
          <title>First Paper</title><link>https://eprint.iacr.org/2026/100</link>
          <dc:creator>Alice</dc:creator><dc:creator>Bob</dc:creator>
          <category>Public-key cryptography</category>
          <pubDate>Thu, 24 Sep 2026 10:00:00 +0000</pubDate>
          <description>Abstract one.</description>
        </item><item>
          <title>Second Paper</title><link>https://eprint.iacr.org/2026/101</link>
        </item></channel></rss>"#;
    Mock::given(method("GET"))
        .and(path("/rss/rss.xml"))
        .and(query_param("order", "recent"))
        .and(query_param("category", "PUBLICKEY"))
        .respond_with(ResponseTemplate::new(200).set_body_string(rss))
        .mount(&h.server)
        .await;

    let out = h
        .run(
            &["feed", "new", "--category", "publickey", "--limit", "1"],
            &[],
        )
        .await;
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "1. First Paper\n   Alice, Bob\n   [Public-key cryptography]\n   https://eprint.iacr.org/2026/100\n   Thu, 24 Sep 2026 10:00:00 +0000\n\n"
    );

    let items = json(
        &h.run(&["--json", "feed", "new", "--category", "publickey"], &[])
            .await,
    );
    assert_eq!(items.as_array().unwrap().len(), 2);
    assert_eq!(items[1]["title"], "Second Paper");
    assert_eq!(items[1]["authors"], serde_json::json!([]));

    let out = h.run(&["--offline", "feed"], &[]).await;
    assert!(!out.status.success());
}

/// `--md` with Markdown already cached by the current converter: served
/// without touching the (absent) model. From another converter: must
/// reconvert, which needs the model, which --offline can't download.
#[tokio::test(flavor = "multi_thread")]
async fn md_uses_cached_markdown_from_the_current_converter_only() {
    let h = Harness::new().await;
    h.serve_paper(&[(V1, V1_UNIX, PDF_V1)]).await;
    assert!(h.run(&["paper", ID], &[]).await.status.success());
    let dir = h.version_dir(V1);
    std::fs::write(dir.join("paper.md"), "# cached").unwrap();
    let meta_path = dir.join("meta.json");
    let mut meta: Value = serde_json::from_slice(&read(&meta_path)).unwrap();

    meta["md_converter"] = "mineru2.5-pro-2605@bff20d4ae2bf".into();
    std::fs::write(&meta_path, meta.to_string()).unwrap();
    let out = h.run(&["--offline", "paper", ID, "--md"], &[]).await;
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("paper.md (mineru2.5-pro-2605@bff20d4ae2bf)"),
        "{}",
        stdout(&out)
    );

    meta["md_converter"] = "some-older-converter".into();
    std::fs::write(&meta_path, meta.to_string()).unwrap();
    let out = h.run(&["--offline", "paper", ID, "--md"], &[]).await;
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("--offline forbids fetching them"),
        "{}",
        stderr(&out)
    );
    assert_eq!(
        read(&dir.join("paper.md")),
        b"# cached",
        "untouched on failure"
    );
}

/// `cache clear --models` with no model downloaded, and a cache with no
/// foreign directories: no errors, and no lines about either.
#[tokio::test(flavor = "multi_thread")]
async fn cache_clear_with_no_model_and_no_foreign_dirs() {
    let h = Harness::new().await;
    h.serve_paper(&[(V1, V1_UNIX, PDF_V1)]).await;
    assert!(h.run(&["paper", ID], &[]).await.status.success());

    let out = h.run(&["cache", "clear", "--models"], &[]).await;
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.starts_with("deleted 1 paper,"), "{text}");
    assert!(!text.contains("left in place"), "{text}");
    assert!(!text.contains("Markdown model"), "{text}");
}

/// `--print` writes exactly the requested artifact to stdout, nothing else,
/// and fails loudly when it doesn't exist.
#[tokio::test(flavor = "multi_thread")]
async fn print_writes_only_the_requested_artifact() {
    let h = Harness::new().await;
    h.serve_paper(&[(V1, V1_UNIX, PDF_V1), (V2, V2_UNIX, PDF_V2)])
        .await;

    let out = h.run(&[ID, "--print", "bib"], &[]).await;
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "@misc{cryptoeprint:2024/463, title = {Security Guidelines}}\n"
    );
    let out = h.run(&[ID, "--print", "abstract"], &[]).await;
    assert_eq!(
        stdout(&out),
        "Fully Homomorphic Encryption is a cryptographic primitive.\n"
    );
    let out = h.run(&[ID, "--print", "pdf-path"], &[]).await;
    let pdf = h.version_dir(V2).join("paper.pdf");
    assert_eq!(stdout(&out), format!("{}\n", pdf.display()));
    assert_eq!(read(&pdf), PDF_V2);

    // An older version has no abstract (the landing page describes only
    // the current one): an error, not empty output.
    let out = h.run(&[ID, "--at", V1, "--print", "abstract"], &[]).await;
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stdout(&out), "");
    assert!(
        stderr(&out).contains("describes only the current version"),
        "{}",
        stderr(&out)
    );

    // Markdown already converted by the current converter prints without
    // needing the model.
    let dir = h.version_dir(V2);
    std::fs::write(dir.join("paper.md"), "# Converted\n\n$x^2$\n").unwrap();
    let meta_path = dir.join("meta.json");
    let mut meta: Value = serde_json::from_slice(&read(&meta_path)).unwrap();
    meta["md_converter"] = "mineru2.5-pro-2605@bff20d4ae2bf".into();
    std::fs::write(&meta_path, meta.to_string()).unwrap();
    let out = h.run(&["--offline", ID, "--print", "md"], &[]).await;
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "# Converted\n\n$x^2$\n");

    let out = h.run(&["--json", ID, "--print", "bib"], &[]).await;
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("can't be combined with --json"),
        "{}",
        stderr(&out)
    );
}
