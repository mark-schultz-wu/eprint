//! PDF → Markdown with MinerU2.5-Pro, a document vision-language model.
//!
//! Each page is rasterized in pure Rust ([`hayro`]) and parsed by MinerU2.5-Pro
//! running on Candle ([`oar_ocr_vl`]): a layout pass, then per-region content
//! extraction, yielding typed blocks with LaTeX math that [`blocks`] turns
//! into Markdown. It was chosen by a blind-graded bake-off on random eprint
//! papers, where it beat MinerU 3's Python pipeline, oar-ocr's ONNX pipeline,
//! pdf2md, and pdf-extract on every paper.
//!
//! Conversion is slow (~35 s/page on an Apple GPU, minutes per page on CPU),
//! so each page's blocks are cached as they finish and an interrupted run
//! resumes where it stopped.

mod blocks;
mod render;
pub mod weights;

use crate::cli::Context;
use anyhow::{anyhow, Context as _, Result};
use oar_ocr_vl::{DocumentBlock, MinerU, MinerUParseOptions, PageParser};
use std::path::{Path, PathBuf};
use std::time::Instant;
use tracing::{debug, info, warn};

/// Identifies the converter (model + pinned weights) that produced a
/// `paper.md`; cached Markdown from anything else gets regenerated.
pub const CONVERTER_ID: &str = "mineru2.5-pro-2605@bff20d4ae2bf";

/// Rasterization resolution fed to the model (what the benchmark used).
const RENDER_DPI: f32 = 150.0;

/// The compute device used when `EPRINT_MD_DEVICE` isn't set: whatever GPU
/// support this binary was built with, else CPU.
pub fn default_device() -> &'static str {
    if cfg!(feature = "cuda") {
        "cuda"
    } else if cfg!(target_os = "macos") {
        "metal"
    } else {
        "cpu"
    }
}

/// Convert `pdf` to Markdown, caching per-page results under `pages_dir`.
pub async fn convert(cx: &Context, pdf: &Path, pages_dir: &Path) -> Result<String> {
    let model_dir = weights::ensure(cx).await?;
    let job = Job {
        pdf: pdf.to_owned(),
        pages_dir: pages_dir.to_owned(),
        model_dir,
        device: cx.cfg.md_device.clone().unwrap_or_else(|| default_device().to_owned()),
        progress: !cx.json,
    };
    tokio::task::spawn_blocking(move || job.run())
        .await
        .context("Markdown conversion task panicked")?
}

struct Job {
    pdf: PathBuf,
    pages_dir: PathBuf,
    model_dir: PathBuf,
    device: String,
    progress: bool,
}

impl Job {
    fn run(self) -> Result<String> {
        let bytes = std::fs::read(&self.pdf).with_context(|| format!("reading {}", self.pdf.display()))?;
        let pdf = hayro::hayro_syntax::Pdf::new(bytes)
            .map_err(|e| anyhow!("could not parse {} as a PDF: {e:?}", self.pdf.display()))?;
        let pages = pdf.pages();
        std::fs::create_dir_all(&self.pages_dir)?;
        let renderer = render::Renderer::new(RENDER_DPI);
        let cache_path = |i: usize| self.pages_dir.join(format!("{:04}.json", i + 1));
        let pending = (0..pages.len()).filter(|&i| read_page_cache(&cache_path(i)).is_none()).count();

        let mut model: Option<MinerU> = None;
        let mut rendered = Vec::with_capacity(pages.len());
        for (i, page) in pages.iter().enumerate() {
            let cache_path = cache_path(i);
            let blocks = match read_page_cache(&cache_path) {
                Some(blocks) => {
                    debug!(page = i + 1, "page blocks cached");
                    blocks
                }
                None => {
                    if model.is_none() {
                        model = Some(self.load_model(pending, pages.len())?);
                    }
                    let model = model.as_ref().expect("loaded above");
                    let start = Instant::now();
                    let image = renderer.page_image(page);
                    let doc = model
                        .parse_page(&image, &MinerUParseOptions::default())
                        .map_err(|e| anyhow!("MinerU failed on page {}: {e}", i + 1))?;
                    for d in &doc.diagnostics {
                        warn!(page = i + 1, stage = %d.stage, block = ?d.block_index, "{}", d.message);
                    }
                    write_page_cache(&cache_path, &doc.blocks)?;
                    if self.progress {
                        eprintln!("  page {}/{} ({:.0}s)", i + 1, pages.len(), start.elapsed().as_secs_f64());
                    }
                    doc.blocks
                }
            };
            rendered.push(blocks::page_to_markdown(&blocks));
        }

        let markdown = blocks::join(rendered);
        if markdown.trim().is_empty() {
            return Err(crate::exit::CommandFailure::EmptyConversion(format!(
                "MinerU2.5-Pro found no content in any of the {} pages of {}",
                pages.len(),
                self.pdf.display(),
            ))
            .into());
        }
        Ok(markdown)
    }

    fn load_model(&self, pending: usize, total: usize) -> Result<MinerU> {
        let device = oar_ocr_vl::utils::parse_device(&self.device)
            .map_err(|e| anyhow!("can't use device {:?} for Markdown conversion: {e}", self.device))?;
        if self.progress {
            let pace = if self.device == "cpu" {
                "CPU only: expect several minutes per page"
            } else {
                "roughly half a minute per page"
            };
            let what = if pending == total {
                format!("{total} pages")
            } else {
                format!("the remaining {pending} of {total} pages")
            };
            eprintln!("Converting {what} to Markdown with MinerU2.5-Pro on {} ({pace}).", self.device);
        }
        let start = Instant::now();
        let model = MinerU::from_dir(&self.model_dir, device)
            .map_err(|e| anyhow!("loading MinerU2.5-Pro from {}: {e}", self.model_dir.display()))?;
        info!(secs = start.elapsed().as_secs_f64(), "model loaded");
        Ok(model)
    }
}

fn read_page_cache(path: &Path) -> Option<Vec<DocumentBlock>> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Write via a temp file so an interrupted run never leaves a truncated page.
fn write_page_cache(path: &Path, blocks: &[DocumentBlock]) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(blocks)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converter_id_tracks_pinned_revision() {
        assert!(CONVERTER_ID.ends_with(&weights::REVISION[..12]));
    }

    #[test]
    fn page_cache_round_trips_and_ignores_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("0001.json");
        assert!(read_page_cache(&p).is_none());
        let blocks = vec![DocumentBlock {
            block_type: "text".into(),
            bbox: [0.1, 0.2, 0.3, 0.4],
            angle: Some(0),
            content: Some("hello".into()),
        }];
        write_page_cache(&p, &blocks).unwrap();
        let back = read_page_cache(&p).unwrap();
        assert_eq!(back[0].content.as_deref(), Some("hello"));
        std::fs::write(&p, b"{truncated").unwrap();
        assert!(read_page_cache(&p).is_none());
    }
}
