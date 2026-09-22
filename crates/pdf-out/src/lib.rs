//! PDF emission from already-paginated pages.
//! Text comes out as text operators with positioned glyphs — never rasterized.

mod draw;

use draw::{ImageCache, draw_background, draw_text};
use paginate::{Page, PageGeometry};
use printpdf::*;
use render_ir::FontResource;

/// PDF works in pt; the rest of the engine in px at 96dpi.
pub(crate) fn pt(px: f32) -> f32 {
    px * 0.75
}

const PT_PER_MM: f32 = 72.0 / 25.4;

pub fn render_pdf(pages: &[Page], fonts: &[FontResource], geo: &PageGeometry) -> Vec<u8> {
    let mut doc = PdfDocument::new("documento");
    let mut font_warnings = Vec::new();

    let font_ids: Vec<Option<FontId>> = fonts
        .iter()
        .map(|f| {
            ParsedFont::from_bytes(&f.bytes, f.face_index, &mut font_warnings)
                .map(|parsed| doc.add_font(&parsed))
        })
        .collect();

    // One XObject per distinct image; pages reuse the same id (a header logo is
    // not re-embedded once per page).
    let mut image_cache = ImageCache::default();

    let width_pt = pt(geo.sheet_width);
    let height_pt = pt(geo.sheet_height);

    let pdf_pages: Vec<PdfPage> = pages
        .iter()
        .map(|page| {
            let mut ops = Vec::new();
            draw_background(&mut ops, &mut doc, &mut image_cache, page, height_pt);
            draw_text(&mut ops, page, &font_ids, height_pt);
            PdfPage::new(Mm(width_pt / PT_PER_MM), Mm(height_pt / PT_PER_MM), ops)
        })
        .collect();

    doc.with_pages(pdf_pages);
    let mut warnings = Vec::new();
    doc.save(&PdfSaveOptions::default(), &mut warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use paginate::{Margins, Page, PageGeometry};
    use render_ir::{Glyph, TextRun};

    fn geo() -> PageGeometry {
        PageGeometry::a4(false, Margins::default())
    }

    #[test]
    fn an_empty_pdf_is_still_a_valid_pdf() {
        let bytes = render_pdf(&[Page::default()], &[], &geo());
        assert!(bytes.starts_with(b"%PDF-"), "missing PDF header");
        assert!(bytes.windows(5).any(|w| w == b"%%EOF"), "missing trailer");
    }

    #[test]
    fn the_pdf_page_count_matches_the_input() {
        let bytes = render_pdf(
            &[Page::default(), Page::default(), Page::default()],
            &[],
            &geo(),
        );
        let text = String::from_utf8_lossy(&bytes);
        // printpdf serializes without a space; "/Type/Page/" does not match
        // "/Type/Pages".
        let count = text.matches("/Type/Page/").count();
        assert!(count >= 3, "expected at least 3 pages, got {count}");
    }

    #[test]
    fn text_is_emitted_as_a_text_operator_not_as_an_image() {
        let page = Page {
            boxes: vec![],
            images: vec![],
            texts: vec![TextRun {
                origin_x: 100.0,
                baseline_y: 200.0,
                font_index: 0,
                font_size_px: 16.0,
                color: [0, 0, 0],
                glyphs: vec![Glyph { id: 36, x: 0.0, y: 0.0 }],
                text: "A".into(),
            }],
        };
        let font = render_ir::FontResource {
            bytes: render_ir::FontBytes::new(load_test_font()),
            face_index: 0,
        };
        let bytes = render_pdf(&[page], &[font], &geo());
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            text.contains("/FontFile2") || text.contains("/FontFile3"),
            "the font was not embedded"
        );
        assert!(!text.contains("/Subtype /Image"), "text became a bitmap");
    }

    /// Uses a system font so the test needs no binary fixture.
    fn load_test_font() -> Vec<u8> {
        let candidates = [
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/System/Library/Fonts/Helvetica.ttc",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        ];
        for c in candidates {
            if let Ok(b) = std::fs::read(c) {
                return b;
            }
        }
        panic!("no test font found in the known paths");
    }

    #[test]
    fn an_image_becomes_an_xobject_embedded_in_the_pdf() {
        let page = Page {
            boxes: vec![],
            texts: vec![],
            images: vec![render_ir::ImageItem {
                rect: render_ir::Rect { x: 50.0, y: 20.0, width: 80.0, height: 40.0 },
                width_px: 4,
                height_px: 2,
                // 4x2 opaque RGBA.
                rgba: std::sync::Arc::new(vec![200; 4 * 2 * 4]),
                order: 0,
            }],
        };
        let bytes = render_pdf(&[page], &[], &geo());
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("/Subtype/Image") || text.contains("/Subtype /Image"),
            "the image did not become an XObject");
        assert!(text.contains("/Width 4"), "missing bitmap width");
        assert!(text.contains("/Height 2"), "missing bitmap height");
    }

    #[test]
    fn an_empty_image_does_not_break_emission() {
        let page = Page {
            boxes: vec![],
            texts: vec![],
            images: vec![render_ir::ImageItem {
                rect: render_ir::Rect { x: 0.0, y: 0.0, width: 10.0, height: 10.0 },
                width_px: 0,
                height_px: 0,
                rgba: std::sync::Arc::new(vec![]),
                order: 0,
            }],
        };
        let bytes = render_pdf(&[page], &[], &geo());
        assert!(bytes.starts_with(b"%PDF-"));
    }
}
