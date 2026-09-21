//! Painting boxes and bitmaps onto the pixmap.

use crate::premultiply;
use paginate::{Page, Painted};
use render_ir::{ImageItem, Rect};
use tiny_skia::{
    FillRule, FilterQuality, Paint, PathBuilder, Pattern, Pixmap, PremultipliedColorU8,
    Rect as SkRect, SpreadMode, Stroke, Transform,
};

/// Boxes and bitmaps, in document paint order — see `paginate::Page::painted`.
pub fn draw_background(pixmap: &mut Pixmap, page: &Page, scale: f32) {
    for item in page.painted() {
        match item {
            Painted::Box(b) => {
                if let Some(c) = b.background {
                    fill_rect(pixmap, b.rect, scale, opaque(c));
                }
                if b.border_width > 0.0
                    && let Some(c) = b.border_color
                {
                    stroke_rect(pixmap, b.rect, b.border_width, scale, opaque(c));
                }
            }
            Painted::Image(img) => draw_image(pixmap, img, scale),
        }
    }
}

fn opaque(c: [u8; 3]) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(c[0], c[1], c[2], 255)
}

fn sk_rect(r: Rect, scale: f32) -> Option<SkRect> {
    SkRect::from_xywh(
        r.x * scale,
        r.y * scale,
        (r.width * scale).max(f32::EPSILON),
        (r.height * scale).max(f32::EPSILON),
    )
}

fn fill_rect(pixmap: &mut Pixmap, r: Rect, scale: f32, c: tiny_skia::Color) {
    let Some(sk) = sk_rect(r, scale) else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color(c);
    paint.anti_alias = true;
    pixmap.fill_rect(sk, &paint, Transform::identity(), None);
}

fn stroke_rect(pixmap: &mut Pixmap, r: Rect, width: f32, scale: f32, c: tiny_skia::Color) {
    let Some(sk) = sk_rect(r, scale) else {
        return;
    };
    let Some(path) = PathBuilder::from_rect(sk).stroke(
        &Stroke {
            width: (width * scale).max(0.1),
            ..Default::default()
        },
        1.0,
    ) else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color(c);
    paint.anti_alias = true;
    pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
}

/// The display-list bitmap is straight RGBA8; `tiny-skia` is premultiplied.
/// Converting here is the inverse of what `render-core` does when it
/// rasterizes SVG.
fn draw_image(target: &mut Pixmap, img: &ImageItem, scale: f32) {
    if img.width_px == 0 || img.height_px == 0 {
        return;
    }
    if img.rgba.len() != img.width_px as usize * img.height_px as usize * 4 {
        return;
    }
    let Some(mut source) = Pixmap::new(img.width_px, img.height_px) else {
        return;
    };
    for (dst, src) in source.pixels_mut().iter_mut().zip(img.rgba.chunks_exact(4)) {
        *dst = PremultipliedColorU8::from_rgba(
            premultiply(src[0], src[3]),
            premultiply(src[1], src[3]),
            premultiply(src[2], src[3]),
            src[3],
        )
        .unwrap_or_else(|| PremultipliedColorU8::from_rgba(0, 0, 0, 0).unwrap());
    }

    let target_width = img.rect.width * scale;
    let target_height = img.rect.height * scale;
    if target_width <= 0.0 || target_height <= 0.0 {
        return;
    }

    // The `Pattern` does the sampling; the clipped rect is the layout box.
    let pattern = Pattern::new(
        source.as_ref(),
        SpreadMode::Pad,
        FilterQuality::Bilinear,
        1.0,
        Transform::from_scale(
            target_width / img.width_px as f32,
            target_height / img.height_px as f32,
        )
        .post_translate(img.rect.x * scale, img.rect.y * scale),
    );
    let Some(sk) = SkRect::from_xywh(
        img.rect.x * scale,
        img.rect.y * scale,
        target_width,
        target_height,
    ) else {
        return;
    };
    let paint = Paint {
        shader: pattern,
        anti_alias: true,
        ..Default::default()
    };
    target.fill_rect(sk, &paint, Transform::identity(), None);
}
