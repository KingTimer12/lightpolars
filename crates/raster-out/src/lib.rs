//! Image (PNG/JPEG) emission from already-paginated pages.
//!
//! The counterpart of `pdf-out`: same `Page`, same `PageGeometry`, same drawing
//! order (boxes, images, text). The difference is that everything here becomes
//! pixels, text included — a screenshot has no selectable text to preserve.
//!
//! Coordinates: `Page` is in px at 96dpi with its origin at the top left of the
//! sheet, which is the bitmap's own orientation. None of the axis flipping the
//! PDF requires.

mod canvas;
mod encode;
mod text;

use paginate::{Page, PageGeometry};
use render_ir::FontResource;
use tiny_skia::Pixmap;

pub use encode::{render_jpeg, render_png};

/// Cap on each side of the generated image, the same rule the SVG rasterizer
/// uses: a long document at a high scale could otherwise ask for hundreds of MB
/// of bitmap.
pub const MAX_SIDE_PX: u32 = 16_384;

/// Screenshot background. Chromium also starts from opaque white when the
/// document declares no background of its own.
const BACKGROUND: [u8; 4] = [255, 255, 255, 255];

/// Draws a page, without encoding it. Exposed so tests can inspect pixels
/// without decoding a PNG first.
pub fn render_pixmap(
    page: &Page,
    fonts: &[FontResource],
    geo: &PageGeometry,
    scale: f32,
) -> Option<Pixmap> {
    let scale = effective_scale(geo, scale)?;
    let width = (geo.sheet_width * scale).round().max(1.0) as u32;
    let height = (geo.sheet_height * scale).round().max(1.0) as u32;

    let mut pixmap = Pixmap::new(width, height)?;
    pixmap.fill(tiny_skia::Color::from_rgba8(
        BACKGROUND[0],
        BACKGROUND[1],
        BACKGROUND[2],
        BACKGROUND[3],
    ));

    canvas::draw_background(&mut pixmap, page, scale);
    text::draw_texts(&mut pixmap, page, fonts, scale);

    Some(pixmap)
}

/// The requested scale, lowered if the bitmap would exceed `MAX_SIDE_PX`.
fn effective_scale(geo: &PageGeometry, scale: f32) -> Option<f32> {
    // Both sides must be valid: checking only the longest would let a
    // zero-width sheet through.
    for side in [geo.sheet_width, geo.sheet_height] {
        if !side.is_finite() || side <= 0.0 {
            return None;
        }
    }
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let longest = geo.sheet_width.max(geo.sheet_height);
    Some(scale.min(MAX_SIDE_PX as f32 / longest))
}

/// Straight-alpha RGBA to premultiplied, the form `tiny-skia` stores.
pub(crate) fn premultiply(component: u8, alpha: u8) -> u8 {
    ((component as u32 * alpha as u32 + 127) / 255) as u8
}
