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
    let mut pixmap = render_pixmap(page, fonts, geo, scale)?;
    let (width, height) = (pixmap.width(), pixmap.height());

    // Encoded from the pixmap's own buffer rather than through
    // `Pixmap::encode_png`, which copies the whole thing before demultiplying:
    // on a full-page screenshot that copy is as large as the canvas and it
    // doubled the peak of the request. PNG wants straight alpha, so the
    // demultiply happens here, in place.
    demultiply_in_place(pixmap.data_mut());

    // Sized up front instead of letting the `Vec` double its way there: the
    // last doubling before it fits allocates a buffer as big as the one it
    // replaces, and on a full-page screenshot that overshoot was megabytes.
    //
    // Half a byte per pixel is where the documents this renders land — text and
    // flat fills compress hard. A wrong guess costs one reallocation, not
    // correctness.
    let mut out = Vec::with_capacity((width as usize * height as usize / 2).max(8 * 1024));
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().ok()?;
    writer.write_image_data(pixmap.data()).ok()?;
    writer.finish().ok()?;
    Some(out)
}

/// Premultiplied RGBA to straight RGBA, over the same buffer.
fn demultiply_in_place(data: &mut [u8]) {
    for px in data.chunks_exact_mut(4) {
        // Fully opaque is the common case by far (the sheet starts opaque
        // white) and needs no work at all.
        if px[3] == 255 {
            continue;
        }
        if px[3] == 0 {
            px[0..3].fill(0);
            continue;
        }
        let Some(c) = tiny_skia::PremultipliedColorU8::from_rgba(px[0], px[1], px[2], px[3]) else {
            continue;
        };
        let d = c.demultiply();
        px[0] = d.red();
        px[1] = d.green();
        px[2] = d.blue();
    }
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
    let mut pixmap = render_pixmap(page, fonts, geo, scale)?;
    let (width, height) = (pixmap.width(), pixmap.height());

    // Demultiplied and then compacted RGBA -> RGB over the same buffer, rather
    // than into a second one. A full-page screenshot is tens of MB, so the
    // extra buffer was the peak of the whole request. The compaction reads
    // ahead of where it writes (4 bytes per pixel against 3), so one pass is
    // safe.
    let pixels = pixmap.width() as usize * pixmap.height() as usize;
    let data = pixmap.data_mut();
    demultiply_in_place(data);
    for i in 0..pixels {
        let rgb = [data[i * 4], data[i * 4 + 1], data[i * 4 + 2]];
        data[i * 3..i * 3 + 3].copy_from_slice(&rgb);
    }
    let rgb = &data[..pixels * 3];

    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(
        &mut std::io::Cursor::new(&mut out),
        quality.clamp(1, 100),
    )
    .encode(rgb, width, height, image::ExtendedColorType::Rgb8)
    .ok()?;
    Some(out)
}
