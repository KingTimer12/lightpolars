//! Encoding a drawn pixmap into image bytes.

use crate::render_pixmap;
use paginate::{Page, PageGeometry};
use render_ir::FontResource;

/// Draws a page and returns PNG bytes.
///
/// `scale` is the `deviceScaleFactor`: 1.0 gives one image px per CSS px, 2.0
/// gives a double-density image. Returns `None` only when the geometry is
/// degenerate (a zero or non-finite side).
pub fn render_png(
    page: &Page,
    fonts: &[FontResource],
    geo: &PageGeometry,
    scale: f32,
) -> Option<Vec<u8>> {
    render_pixmap(page, fonts, geo, scale)?.encode_png().ok()
}

/// Draws a page and returns JPEG bytes.
///
/// JPEG has no alpha channel: the drawing already starts from an opaque white
/// background, so dropping the alpha (255 across the sheet) is enough.
pub fn render_jpeg(
    page: &Page,
    fonts: &[FontResource],
    geo: &PageGeometry,
    scale: f32,
    quality: u8,
) -> Option<Vec<u8>> {
    let pixmap = render_pixmap(page, fonts, geo, scale)?;
    let mut rgb = Vec::with_capacity(pixmap.width() as usize * pixmap.height() as usize * 3);
    for px in pixmap.pixels() {
        let c = px.demultiply();
        rgb.extend_from_slice(&[c.red(), c.green(), c.blue()]);
    }

    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(
        &mut std::io::Cursor::new(&mut out),
        quality.clamp(1, 100),
    )
    .encode(&rgb, pixmap.width(), pixmap.height(), image::ExtendedColorType::Rgb8)
    .ok()?;
    Some(out)
}
