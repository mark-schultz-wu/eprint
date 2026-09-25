//! Rasterize PDF pages for the model, in pure Rust.

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::page::Page;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{RenderCache, RenderSettings};
use image::RgbImage;

pub struct Renderer {
    scale: f32,
    settings: InterpreterSettings,
}

impl Renderer {
    pub fn new(dpi: f32) -> Self {
        // PDF user space is 72 units per inch.
        Self {
            scale: dpi / 72.0,
            settings: InterpreterSettings::default(),
        }
    }

    /// Render one page onto a white background as RGB.
    pub fn page_image(&self, page: &Page<'_>) -> RgbImage {
        let pixmap = hayro::render(
            page,
            &RenderCache::new(),
            &self.settings,
            &RenderSettings {
                x_scale: self.scale,
                y_scale: self.scale,
                bg_color: WHITE,
                ..Default::default()
            },
        );
        let (w, h) = (u32::from(pixmap.width()), u32::from(pixmap.height()));
        // Premultiplied RGBA over an opaque background is plain RGBA; drop alpha.
        let rgb: Vec<u8> = pixmap
            .data_as_u8_slice()
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|&[r, g, b, _]| [r, g, b])
            .collect();
        RgbImage::from_raw(w, h, rgb).expect("pixmap is exactly w*h RGBA pixels")
    }
}
