//! Rasterizing and compositing glyph runs.

use crate::premultiply;
use paginate::Page;
use render_ir::{FontResource, TextRun};
use swash::FontRef;
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::zeno::{Format, Vector};
use tiny_skia::{Pixmap, PremultipliedColorU8};

/// Draws every run, building one scaler per stretch of consecutive runs that
/// share a font and a size.
///
/// Building a scaler parses the font tables again, and a text-heavy document is
/// hundreds of runs that almost all share the same face and size — a paragraph
/// is one run per line. Grouping only *consecutive* runs (rather than sorting
/// by font) keeps the painting order exactly as it was, which matters wherever
/// two runs overlap.
pub fn draw_texts(pixmap: &mut Pixmap, page: &Page, fonts: &[FontResource], scale: f32) {
    let mut ctx = ScaleContext::new();
    let runs = &page.texts;
    let mut start = 0;

    while start < runs.len() {
        let key = |r: &TextRun| (r.font_index, r.font_size_px.to_bits());
        let mut end = start + 1;
        while end < runs.len() && key(&runs[end]) == key(&runs[start]) {
            end += 1;
        }
        draw_group(pixmap, &mut ctx, &runs[start..end], fonts, scale);
        start = end;
    }
}

/// Runs sharing one font and one size, so one scaler serves all of them.
fn draw_group(
    pixmap: &mut Pixmap,
    ctx: &mut ScaleContext,
    runs: &[TextRun],
    fonts: &[FontResource],
    scale: f32,
) {
    let Some(first) = runs.first() else {
        return;
    };
    let Some(resource) = fonts.get(first.font_index) else {
        return;
    };
    let Some(font) = FontRef::from_index(&resource.bytes, resource.face_index) else {
        return;
    };

    let size = first.font_size_px * scale;
    if !size.is_finite() || size <= 0.0 {
        return;
    }
    let mut scaler = ctx.builder(font).size(size).hint(false).build();

    for run in runs {
        draw_run(pixmap, &mut scaler, run, scale);
    }
}

fn draw_run(pixmap: &mut Pixmap, scaler: &mut swash::scale::Scaler, run: &TextRun, scale: f32) {
    for g in &run.glyphs {
        let x = (run.origin_x + g.x) * scale;
        let y = (run.baseline_y + g.y) * scale;
        // The integer part places the bitmap; the fractional part goes to the
        // rasterizer, otherwise the text "dances" half a pixel between glyphs.
        let (xi, yi) = (x.floor(), y.floor());
        let Some(mask) = Render::new(&[
            Source::ColorOutline(0),
            Source::ColorBitmap(StrikeWith::BestFit),
            Source::Outline,
        ])
        .format(Format::Alpha)
        .offset(Vector::new(x - xi, y - yi))
        .render(scaler, g.id)
        else {
            continue;
        };

        blend_mask(
            pixmap,
            &mask.data,
            mask.placement.width,
            mask.placement.height,
            xi as i32 + mask.placement.left,
            yi as i32 - mask.placement.top,
            run.color,
        );
    }
}

/// Blends an 8-bit alpha mask over the pixmap in the given color (source-over).
fn blend_mask(
    pixmap: &mut Pixmap,
    mask: &[u8],
    width: u32,
    height: u32,
    left: i32,
    top: i32,
    color: [u8; 3],
) {
    if width == 0 || height == 0 || mask.len() < (width * height) as usize {
        return;
    }
    let (target_width, target_height) = (pixmap.width() as i32, pixmap.height() as i32);
    let target = pixmap.pixels_mut();

    for row in 0..height as i32 {
        let y = top + row;
        if y < 0 || y >= target_height {
            continue;
        }
        for col in 0..width as i32 {
            let x = left + col;
            if x < 0 || x >= target_width {
                continue;
            }
            let a = mask[(row as u32 * width + col as u32) as usize];
            if a == 0 {
                continue;
            }
            let idx = (y * target_width + x) as usize;
            let old = target[idx];
            let inverse = 255 - a as u32;
            let mix = |new: u8, previous: u8| -> u8 {
                ((premultiply(new, a) as u32) + (previous as u32 * inverse + 127) / 255).min(255)
                    as u8
            };
            target[idx] = PremultipliedColorU8::from_rgba(
                mix(color[0], old.red()),
                mix(color[1], old.green()),
                mix(color[2], old.blue()),
                (a as u32 + (old.alpha() as u32 * inverse + 127) / 255).min(255) as u8,
            )
            .unwrap_or(old);
        }
    }
}
