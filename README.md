# eprint

A CLI for fetching IACR ePrint papers and converting them to Markdown with
math as LaTeX. It's one self-contained Rust binary: no Python, and no external
tools at runtime.

## Install

```
cargo install --path .                  # Metal GPU acceleration on macOS
cargo install --path . --features cuda  # NVIDIA GPU (needs a CUDA toolkit)
```

Requires Rust 1.95+.

## Usage

```
eprint 2024/463                         # metadata, PDF, BibTeX, abstract → cache
eprint 2024/463 --md                    # … plus Markdown (see below)
eprint 2024/463 --at 20240319T143540Z   # a specific (e.g. older) version
eprint 2024/463 --select-version        # pick a version interactively
eprint sync                             # OAI-PMH refresh of cached papers' versions
eprint feed [new|updates] --category publickey
eprint cache {path,list,clear}          # clear keeps the Markdown model; --models drops it
```

`eprint <id> ...` is shorthand for `eprint paper <id> ...`.

Global flags: `--offline`, `--json`, `-v`/`-vv`/`-vvv`, `--log-format=json`.
Paper ids can be `2024/463`, `2024-463`, or a full eprint URL. Scripts can
tell failures apart by exit code: 2 (no version resolved), 3 (PDF
unavailable), 4 (empty conversion), 1 (anything else).

## Markdown conversion

`--md` runs [MinerU2.5-Pro](https://huggingface.co/opendatalab/MinerU2.5-Pro-2605-1.2B),
a 1.2B-parameter document vision-language model, in-process via
[oar-ocr-vl](https://github.com/GreatV/oar-ocr) (Candle). Each page is
rasterized with [hayro](https://github.com/LaurenzV/hayro) (pure Rust), then
parsed into blocks with LaTeX math, which become Markdown.

- **Model download:** 2.2 GB on first use, into
  `<cache>/models/`. It's pinned to a specific commit and verified by
  SHA-256, and an interrupted download resumes.
- **Speed:** about 30–60 s per page on an Apple M2 Pro GPU, and several
  minutes per page on CPU only. Pages are cached as they finish, so an
  interrupted conversion resumes where it stopped.
- **Device:** the best one the binary was built with (Metal on macOS, CUDA
  with `--features cuda`, else CPU). Override with `EPRINT_MD_DEVICE`.

This converter was picked by a blind-graded comparison on 10 random eprint
papers (30 pages). It scored 4.4/5, vs 3.4 for MinerU 3's Python pipeline and
about 2 for pdf-extract, pdf2md, and oar-ocr's ONNX pipeline.

## Configuration

Settings come from environment variables; there's no config file.

| Variable | Meaning |
|---|---|
| `EPRINT_CACHE_DIR` | cache root (default: the OS cache dir, e.g. `~/Library/Caches/eprint` on macOS, `~/.cache/eprint` on Linux) |
| `EPRINT_CONTACT` | contact appended to the outbound `User-Agent` |
| `EPRINT_MIN_INTERVAL_S` | minimum seconds between eprint requests (default `2.0`) |
| `EPRINT_MD_DEVICE` | Markdown converter device: `metal`, `cuda`, or `cpu` |
| `EPRINT_AUTO_SYNC` | auto-run OAI-PMH sync when the cache is stale (default `true`) |
| `EPRINT_SYNC_STALE_HOURS` | hours before the cache counts as stale (default `24`) |

## Network etiquette

eprint.iacr.org rate-limits per IP (HTTP 429 after about 20 rapid requests).
The CLI paces itself (one request per `EPRINT_MIN_INTERVAL_S`, small bursts)
and backs off and retries on 429.

## Development

```
cargo test                  # unit tests + tests/e2e.rs
cargo fmt --check && cargo clippy --all-targets -- -D warnings
cargo mutants -j 4          # mutation testing (cargo install cargo-mutants)
```

`tests/e2e.rs` runs the real binary against a local fake eprint server
(`wiremock`), pointed there with `EPRINT_BASE_URL`, so the full fetch / version
/ sync / offline flows are tested without touching eprint.iacr.org. CI
(`.github/workflows/ci.yml`) runs fmt, clippy, and the tests on Linux and
macOS, and checks the build on the minimum Rust version.

## Notes

Personal project by Mark Schultz-Wu. **Not** an officially endorsed
Fabric Cryptography project, even though I use it for cryptography work.
