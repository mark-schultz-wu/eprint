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
use image::RgbImage;
use oar_ocr_vl::{DocumentBlock, MinerU, MinerUParseOptions, PageParser};
use std::path::{Path, PathBuf};
use std::time::Instant;
use tracing::{debug, info, warn};

/// Identifies the converter (model + pinned weights) that produced a
/// `paper.md`; cached Markdown from anything else gets regenerated.
pub const CONVERTER_ID: &str = "mineru2.5-pro-2605@bff20d4ae2bf";

/// Rasterization resolution fed to the model (what the benchmark used).
const RENDER_DPI: f32 = 150.0;

/// How many of a page's layout regions the model reads per batch (the
/// library default is 2). Output is byte-identical at any batch size
/// (checked from 2 to 32). On an M2 Pro, 2022/1160 (6 pages, f16) took
/// 237 s at 2, 192 s at 8, 184 s at 16, and 178 s at 32, and peak memory
/// stayed ~5.9 GB throughout; 16 takes most of the gain.
const REGION_BATCH_SIZE: usize = 16;

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

/// The result of converting a PDF.
#[derive(Debug)]
pub struct Converted {
    pub markdown: String,
    /// 1-based numbers of pages that failed to convert. Each appears in
    /// `markdown` as a placeholder comment, and isn't cached, so the next
    /// conversion retries just those pages.
    pub failed_pages: Vec<usize>,
}

/// Convert `pdf` to Markdown, caching per-page results under `pages_dir`.
pub async fn convert(cx: &Context, pdf: &Path, pages_dir: &Path) -> Result<Converted> {
    let model_dir = weights::ensure(cx).await?;
    let job = Job {
        pdf: pdf.to_owned(),
        pages_dir: pages_dir.to_owned(),
        model_dir,
        device: cx
            .cfg
            .md_device
            .clone()
            .unwrap_or_else(|| default_device().to_owned()),
        progress: !cx.json,
    };
    tokio::task::spawn_blocking(move || job.run())
        .await
        .context("Markdown conversion task panicked")?
}

/// Turns one rendered page into blocks: MinerU in production, fakes in tests.
trait PageModel {
    fn parse(&self, page: usize, image: &RgbImage) -> Result<Vec<DocumentBlock>>;
}

impl PageModel for MinerU {
    fn parse(&self, page: usize, image: &RgbImage) -> Result<Vec<DocumentBlock>> {
        let doc = self
            .parse_page(
                image,
                &MinerUParseOptions {
                    region_batch_size: REGION_BATCH_SIZE,
                    ..MinerUParseOptions::default()
                },
            )
            .map_err(|e| anyhow!("{e}"))?;
        for d in &doc.diagnostics {
            warn!(page, stage = %d.stage, block = ?d.block_index, "{}", d.message);
        }
        Ok(doc.blocks)
    }
}

struct Job {
    pdf: PathBuf,
    pages_dir: PathBuf,
    model_dir: PathBuf,
    device: String,
    progress: bool,
}

impl Job {
    fn run(self) -> Result<Converted> {
        let bytes =
            std::fs::read(&self.pdf).with_context(|| format!("reading {}", self.pdf.display()))?;
        convert_pages(
            bytes,
            &self.pages_dir,
            |pending, total| self.load_model(pending, total),
            self.progress,
        )
        .with_context(|| format!("converting {}", self.pdf.display()))
    }

    fn load_model(&self, pending: usize, total: usize) -> Result<MinerU> {
        let device = oar_ocr_vl::utils::parse_device(&self.device).map_err(|e| {
            anyhow!(
                "can't use device {:?} for Markdown conversion: {e}",
                self.device
            )
        })?;
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
            eprintln!(
                "Converting {what} to Markdown with MinerU2.5-Pro on {} ({pace}).",
                self.device
            );
        }
        let start = Instant::now();
        let mut runtime = oar_ocr_vl::RuntimeConfig::new(device);
        let user_override = std::env::var_os("OAR_VL_DTYPE").is_some();
        if let Some(dtype) = compute_dtype(runtime.device.is_metal(), user_override) {
            runtime = runtime.with_dtype(dtype);
        }
        let model = MinerU::from_dir_with_runtime(&self.model_dir, runtime).map_err(|e| {
            anyhow!(
                "loading MinerU2.5-Pro from {}: {e}",
                self.model_dir.display()
            )
        })?;
        info!(secs = start.elapsed().as_secs_f64(), "model loaded");
        Ok(model)
    }
}

/// Render and convert each page of `pdf_bytes`, reusing pages cached in
/// `pages_dir`. `load` builds the model; it's called at most once, and only
/// if some page isn't cached. A page that fails to convert becomes a
/// placeholder (and stays uncached for a retry) instead of sinking the
/// whole paper; failing on every page is an error.
fn convert_pages<M: PageModel>(
    pdf_bytes: Vec<u8>,
    pages_dir: &Path,
    load: impl FnOnce(usize, usize) -> Result<M>,
    progress: bool,
) -> Result<Converted> {
    let pdf = hayro::hayro_syntax::Pdf::new(pdf_bytes)
        .map_err(|e| anyhow!("could not parse the PDF: {e:?}"))?;
    let pages = pdf.pages();
    let total = pages.len();
    std::fs::create_dir_all(pages_dir)?;
    let renderer = render::Renderer::new(RENDER_DPI);
    let cache_path = |i: usize| pages_dir.join(format!("{:04}.json", i + 1));
    let pending = (0..total)
        .filter(|&i| read_page_cache(&cache_path(i)).is_none())
        .count();

    let mut load = Some(load);
    let mut model: Option<M> = None;
    let mut rendered = Vec::with_capacity(total);
    let mut failed_pages = Vec::new();
    let mut first_error = None;
    for (i, page) in pages.iter().enumerate() {
        let number = i + 1;
        let cache_path = cache_path(i);
        if let Some(blocks) = read_page_cache(&cache_path) {
            debug!(page = number, "page blocks cached");
            rendered.push(blocks::page_to_markdown(&blocks));
            continue;
        }
        if model.is_none() {
            let load = load.take().expect("the model is loaded at most once");
            model = Some(load(pending, total)?);
        }
        let model = model.as_ref().expect("loaded above");
        let start = Instant::now();
        match model.parse(number, &renderer.page_image(page)) {
            Ok(blocks) => {
                write_page_cache(&cache_path, &blocks)?;
                if progress {
                    eprintln!(
                        "  page {number}/{total} ({:.0}s)",
                        start.elapsed().as_secs_f64()
                    );
                }
                rendered.push(blocks::page_to_markdown(&blocks));
            }
            Err(e) => {
                let reason = format!("{e:#}").replace("--", "- -"); // keep the comment valid
                warn!(page = number, "page failed to convert: {reason}");
                if progress {
                    eprintln!("  page {number}/{total} FAILED: {reason}");
                }
                rendered.push(vec![format!(
                    "<!-- page {number} could not be converted: {reason} -->"
                )]);
                failed_pages.push(number);
                first_error.get_or_insert(e);
            }
        }
    }

    if failed_pages.len() == total {
        let e = first_error.expect("every page failed, so there's an error");
        return Err(e.context(format!("MinerU2.5-Pro failed on all {total} pages")));
    }
    let markdown = blocks::join(rendered);
    if failed_pages.is_empty() && markdown.trim().is_empty() {
        return Err(crate::exit::CommandFailure::EmptyConversion(format!(
            "MinerU2.5-Pro found no content in any of the {total} pages"
        ))
        .into());
    }
    Ok(Converted {
        markdown,
        failed_pages,
    })
}

/// The compute dtype to load the model with, or `None` for oar-ocr-vl's
/// automatic choice (which honors an `OAR_VL_DTYPE` override).
///
/// On Apple GPUs the automatic choice is bf16, but f16 measured both faster
/// and much closer to full precision (it keeps more mantissa bits), on an
/// M2 Pro:
///
/// | paper (pages)    | time: bf16 / f16 / f32 | lines differing from f32: bf16 / f16 |
/// |------------------|------------------------|--------------------------------------|
/// | 2022/1160 (6)    | 255 / 237 / 306 s (b2) | 1 / 0                                |
/// | 2020/020 (13)    | 580 / 543 / 667 s (b16)| 18 / 2                               |
///
/// CUDA and CPU keep the automatic choice: not benchmarked here.
fn compute_dtype(is_metal: bool, user_override: bool) -> Option<candle_core::DType> {
    (is_metal && !user_override).then_some(candle_core::DType::F16)
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

    use std::cell::RefCell;
    use std::rc::Rc;

    /// Returns one text block per page, failing on the pages in `fail`, and
    /// logs which pages it was asked for.
    struct Fake {
        fail: Vec<usize>,
        calls: Rc<RefCell<Vec<usize>>>,
        empty: bool,
    }

    impl PageModel for Fake {
        fn parse(&self, page: usize, image: &RgbImage) -> Result<Vec<DocumentBlock>> {
            assert_eq!(
                image.dimensions(),
                (416, 208),
                "rendered at 150 dpi (floored)"
            );
            self.calls.borrow_mut().push(page);
            if self.fail.contains(&page) {
                anyhow::bail!("GPU fell over -- on page {page}");
            }
            let content = (!self.empty).then(|| format!("Text of page {page}."));
            Ok(vec![DocumentBlock {
                block_type: "text".into(),
                bbox: [0.0; 4],
                angle: None,
                content,
            }])
        }
    }

    fn fake(fail: &[usize]) -> (Fake, Rc<RefCell<Vec<usize>>>) {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let f = Fake {
            fail: fail.to_vec(),
            calls: calls.clone(),
            empty: false,
        };
        (f, calls)
    }

    #[test]
    fn converts_every_page_in_order_and_caches_them() {
        let dir = tempfile::tempdir().unwrap();
        let (model, calls) = fake(&[]);
        let mut loaded = None;
        let out = convert_pages(
            render::test_pdf(3),
            dir.path(),
            |pending, total| {
                loaded = Some((pending, total));
                Ok(model)
            },
            false,
        )
        .unwrap();
        assert_eq!(
            out.markdown,
            "Text of page 1.\n\nText of page 2.\n\nText of page 3.\n"
        );
        assert!(out.failed_pages.is_empty());
        assert_eq!(loaded, Some((3, 3)));
        assert_eq!(*calls.borrow(), [1, 2, 3]);
        for n in ["0001", "0002", "0003"] {
            assert!(dir.path().join(format!("{n}.json")).exists(), "{n}");
        }

        // Everything is cached now: the model isn't even loaded.
        let again = convert_pages(
            render::test_pdf(3),
            dir.path(),
            |_, _| -> Result<Fake> { panic!("the model must not be loaded") },
            false,
        )
        .unwrap();
        assert_eq!(again.markdown, out.markdown);
    }

    /// Regression: one bad page used to abort the whole conversion, and
    /// every retry hit it again.
    #[test]
    fn a_failing_page_becomes_a_placeholder_and_only_it_is_retried() {
        let dir = tempfile::tempdir().unwrap();
        let (model, _) = fake(&[2]);
        let out = convert_pages(render::test_pdf(3), dir.path(), |_, _| Ok(model), false).unwrap();
        assert_eq!(out.failed_pages, [2]);
        assert_eq!(
            out.markdown,
            "Text of page 1.\n\n\
             <!-- page 2 could not be converted: GPU fell over - - on page 2 -->\n\n\
             Text of page 3.\n"
        );
        assert!(
            !dir.path().join("0002.json").exists(),
            "a failed page isn't cached"
        );

        let (model, calls) = fake(&[]);
        let mut loaded = None;
        let out = convert_pages(
            render::test_pdf(3),
            dir.path(),
            |pending, total| {
                loaded = Some((pending, total));
                Ok(model)
            },
            false,
        )
        .unwrap();
        assert_eq!(loaded, Some((1, 3)), "one page left to convert");
        assert_eq!(*calls.borrow(), [2]);
        assert!(out.failed_pages.is_empty());
        assert!(out.markdown.contains("Text of page 2."));
    }

    #[test]
    fn failing_on_every_page_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let (model, _) = fake(&[1, 2]);
        let err =
            convert_pages(render::test_pdf(2), dir.path(), |_, _| Ok(model), false).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("failed on all 2 pages"), "{msg}");
        assert!(msg.contains("GPU fell over"), "{msg}");
    }

    #[test]
    fn no_content_on_any_page_is_an_empty_conversion() {
        let dir = tempfile::tempdir().unwrap();
        let (mut model, _) = fake(&[]);
        model.empty = true;
        let err =
            convert_pages(render::test_pdf(2), dir.path(), |_, _| Ok(model), false).unwrap_err();
        assert!(matches!(
            err.downcast_ref::<crate::exit::CommandFailure>(),
            Some(crate::exit::CommandFailure::EmptyConversion(_))
        ));
    }

    #[test]
    fn a_model_that_fails_to_load_or_a_bad_pdf_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = convert_pages(
            render::test_pdf(1),
            dir.path(),
            |_, _| -> Result<Fake> { anyhow::bail!("no GPU") },
            false,
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "no GPU");
        let (model, _) = fake(&[]);
        let err =
            convert_pages(b"not a pdf".to_vec(), dir.path(), |_, _| Ok(model), false).unwrap_err();
        assert!(err.to_string().contains("could not parse the PDF"), "{err}");
    }

    #[test]
    fn metal_uses_f16_unless_the_user_chose_a_dtype() {
        assert_eq!(compute_dtype(true, false), Some(candle_core::DType::F16));
        assert_eq!(compute_dtype(true, true), None, "OAR_VL_DTYPE wins");
        assert_eq!(compute_dtype(false, false), None, "CUDA/CPU: automatic");
    }

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
