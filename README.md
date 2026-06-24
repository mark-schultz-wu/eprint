# eprint

A CLI for fetching and converting IACR ePrint papers.

This is a Cargo workspace with two crates:

- **`eprint/`** — the user-facing CLI. Fetches PDFs, BibTeX, and abstracts
  from `eprint.iacr.org`; converts PDFs to Markdown via `papermd`.
- **`papermd/`** — a small crate that converts academic PDFs to Markdown.
  Has two backends:
  - `LocalConverter` — subprocesses
    [MinerU](https://github.com/opendatalab/MinerU) via `uv`. Needs **only
    `uv`** on `PATH` — it bootstraps an ephemeral Python + MinerU on first use
    (no system Python/pip/MinerU install). First run pulls ~1–2 GB of model
    weights into uv's cache.
  - `RemoteConverter` — HTTP client that talks to a MinerU FastAPI server
    (or any server speaking the same simple `POST /v1/convert` API).

## Dependencies

The base tool is self-contained — no runtime dependencies:

- **Default (`text` quality)** uses the pure-Rust `pdf-extract` crate. No
  Python, no network, nothing to install.
- **`--md ml` (high-fidelity, math/tables)** needs an ML backend:
  - *local* (`EPRINT_ML_BACKEND=local`, default): just `uv`. Missing uv yields
    an actionable error, not a crash.
  - *remote* (`EPRINT_ML_BACKEND=remote`): set `EPRINT_ML_ENDPOINT`; no local
    Python/uv needed.

PDFs themselves are **not** fetched over HTTP — `eprint.iacr.org` is behind a
Cloudflare challenge (403). They arrive via the downloads dir
(`EPRINT_DOWNLOADS_DIR`, default `~/Downloads`), delivered by the companion
MacBook watcher; an S3 source is planned. See `eprint/src/source.rs`.

## CLI

```
eprint fetch    2024/463                # pdf + bib + abstract → cache
eprint show     2024/463                # print metadata (human / --json)
eprint convert  2024/463                # markdown, --quality=text default
                                        # --quality=ml for slow ML pipeline
eprint refresh  2024/463                # re-fetch all artifacts
eprint check    2024/463                # report staleness
eprint cache    {path,clear,list}
```

Global flags: `--offline`, `--json`, `-v`/`-vv`/`-vvv`, `--log-format=json`,
`NO_COLOR` honored.

## Status

Early scaffolding.

## Notes

Personal project by Mark Schultz-Wu. **Not** an officially endorsed
Fabric Cryptography project, even though I use it for cryptography work.
