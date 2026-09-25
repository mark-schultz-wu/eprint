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

/// A PDF of `pages` pages, each 200×100 pt with a black rectangle at
/// x 10–60, y 10–40 (PDF coordinates: origin bottom-left).
#[cfg(test)]
pub(crate) fn test_pdf(pages: usize) -> Vec<u8> {
    let content = "0 0 0 rg 10 10 50 30 re f";
    let kids: Vec<String> = (0..pages).map(|i| format!("{} 0 R", 3 + 2 * i)).collect();
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        format!(
            "<< /Type /Pages /Kids [{}] /Count {pages} >>",
            kids.join(" ")
        ),
    ];
    for i in 0..pages {
        let contents = 4 + 2 * i;
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents {contents} 0 R >>"
        ));
        objects.push(format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        ));
    }
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for off in offsets {
        pdf.extend(format!("{off:010} 00000 n \n").as_bytes());
    }
    pdf.extend(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    pdf
}

#[cfg(test)]
mod tests {
    use super::*;
    use hayro::hayro_syntax::Pdf;

    #[test]
    fn renders_at_the_requested_dpi_on_white() {
        let pdf = Pdf::new(test_pdf(1)).unwrap();
        let page = &pdf.pages()[0];

        let img = Renderer::new(144.0).page_image(page); // 2 px per pt
        assert_eq!(img.dimensions(), (400, 200));
        assert_eq!(img.get_pixel(2, 2).0, [255, 255, 255], "white background");
        // The rectangle spans x 20–120 px, y 120–180 px (image origin top-left).
        assert_eq!(img.get_pixel(70, 150).0, [0, 0, 0]);
        assert_eq!(
            img.get_pixel(70, 100).0,
            [255, 255, 255],
            "above the rectangle"
        );

        assert_eq!(
            Renderer::new(72.0).page_image(page).dimensions(),
            (200, 100)
        );
    }
}
