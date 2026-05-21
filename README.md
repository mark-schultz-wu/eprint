# eprint

A CLI for fetching and converting IACR ePrint papers.

This is a Cargo workspace with two crates:

- **`eprint/`** — the user-facing CLI. Fetches PDFs, BibTeX, and abstracts
  from `eprint.iacr.org`; converts PDFs to Markdown via `papermd`.
- **`papermd/`** — a small crate that converts academic PDFs to Markdown.
  Has two backends:
  - `LocalConverter` — subprocesses
    [MinerU](https://github.com/opendatalab/MinerU) via `uv` (requires
    Python + uv on the machine).
  - `RemoteConverter` — HTTP client that talks to a MinerU FastAPI server
    (or any server speaking the same simple `POST /v1/convert` API).

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
