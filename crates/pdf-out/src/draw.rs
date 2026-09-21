//! Turning display-list items into PDF content-stream operators.
//!
//! PDF has its origin at the bottom left, the display list at the top left, so
//! every y here is mirrored against the sheet height.

use crate::pt;
use paginate::{Page, Painted};
use printpdf::*;
use std::collections::HashMap;

/// Identity of an already-embedded bitmap: its dimensions plus the address of
/// the shared buffer, which is what the display list's `Arc` preserves across
/// pages.
type ImageKey = (u32, u32, usize);

#[derive(Default)]
pub struct ImageCache(HashMap<ImageKey, XObjectId>);

/// Boxes and bitmaps, in document paint order — see `paginate::Page::painted`.
pub fn draw_background(
    ops: &mut Vec<Op>,
    doc: &mut PdfDocument,
    cache: &mut ImageCache,
    page: &Page,
    height_pt: f32,
) {
    for item in page.painted() {
        match item {
            Painted::Box(b) => draw_box(ops, b, height_pt),
            Painted::Image(img) => draw_image(ops, doc, cache, img, height_pt),
        }
    }
}

fn draw_box(ops: &mut Vec<Op>, b: &render_ir::BoxItem, height_pt: f32) {
    let shape = |mode: PaintMode| {
        if b.has_radius() {
            outline_op(b, height_pt, mode)
        } else {
            rect_op(b.rect, height_pt, mode)
        }
    };
    if let Some(color) = b.background {
        ops.push(Op::SetFillColor { col: rgb_color(color) });
        ops.push(shape(PaintMode::Fill));
    }
    if b.border_width > 0.0
        && let Some(color) = b.border_color
    {
        ops.push(Op::SetOutlineColor { col: rgb_color(color) });
        ops.push(Op::SetOutlineThickness { pt: Pt(pt(b.border_width)) });
        ops.push(shape(PaintMode::Stroke));
    }
}

/// A rounded outline as a polygon whose curves are cubic beziers.
///
/// printpdf reads a run of two points flagged `bezier` followed by a plain one
/// as the two handles and the end of a curve (`serialize.rs`), so the control
/// points are emitted in that order.
fn outline_op(b: &render_ir::BoxItem, height_pt: f32, mode: PaintMode) -> Op {
    use render_ir::PathCmd;

    let at = |p: [f32; 2]| Point { x: Pt(pt(p[0])), y: Pt(height_pt - pt(p[1])) };
    let mut points = Vec::new();
    for cmd in b.outline() {
        match cmd {
            PathCmd::MoveTo(p) | PathCmd::LineTo(p) => {
                points.push(LinePoint { p: at(p), bezier: false });
            }
            PathCmd::CurveTo(c1, c2, p) => {
                points.push(LinePoint { p: at(c1), bezier: true });
                points.push(LinePoint { p: at(c2), bezier: true });
                points.push(LinePoint { p: at(p), bezier: false });
            }
        }
    }

    Op::DrawPolygon {
        polygon: Polygon {
            rings: vec![PolygonRing { points }],
            mode,
            winding_order: WindingOrder::NonZero,
        },
    }
}

fn draw_image(
    ops: &mut Vec<Op>,
    doc: &mut PdfDocument,
    cache: &mut ImageCache,
    img: &render_ir::ImageItem,
    height_pt: f32,
) {
    if img.width_px == 0 || img.height_px == 0 || img.rgba.is_empty() {
        return;
    }
    let key = (
        img.width_px,
        img.height_px,
        std::sync::Arc::as_ptr(&img.rgba) as usize,
    );
    let id = cache.0.entry(key).or_insert_with(|| {
        doc.add_image(&RawImage {
            pixels: RawImageData::U8(img.rgba.as_ref().clone()),
            width: img.width_px as usize,
            height: img.height_px as usize,
            data_format: RawImageFormat::RGBA8,
            tag: Vec::new(),
        })
    });

    // At dpi = 72 the UseXobject auto-scaling puts the bitmap at 1px = 1pt;
    // scale_x/y take it from there to the layout box.
    ops.push(Op::UseXobject {
        id: id.clone(),
        transform: XObjectTransform {
            translate_x: Some(Pt(pt(img.rect.x))),
            translate_y: Some(Pt(height_pt - pt(img.rect.y) - pt(img.rect.height))),
            scale_x: Some(pt(img.rect.width) / img.width_px as f32),
            scale_y: Some(pt(img.rect.height) / img.height_px as f32),
            rotate: None,
            dpi: Some(72.0),
            no_auto_scale: false,
        },
    });
}

pub fn draw_text(ops: &mut Vec<Op>, page: &Page, font_ids: &[Option<FontId>], height_pt: f32) {
    for run in &page.texts {
        let Some(Some(font_id)) = font_ids.get(run.font_index) else {
            continue;
        };
        if run.glyphs.is_empty() {
            continue;
        }
        let handle = PdfFontHandle::External(font_id.clone());

        ops.push(Op::StartTextSection);
        ops.push(Op::SetFillColor { col: rgb_color(run.color) });
        ops.push(Op::SetFont { font: handle.clone(), size: Pt(pt(run.font_size_px)) });

        // One glyph at a time, each with its own text matrix: shaping already
        // gave the absolute position of every glyph inside the run, so there is
        // no advance to recompute here.
        let mut chars = run.text.chars();
        for g in &run.glyphs {
            let x = pt(run.origin_x + g.x);
            let y = height_pt - pt(run.baseline_y + g.y);
            ops.push(Op::SetTextMatrix {
                matrix: TextMatrix::Translate(Pt(x), Pt(y)),
            });
            ops.push(Op::ShowText {
                items: vec![TextItem::GlyphIds(vec![Codepoint {
                    gid: g.id,
                    offset: 0.0,
                    // Feeds ToUnicode so the text comes out selectable.
                    cid: chars.next().map(String::from),
                }])],
            });
        }

        ops.push(Op::EndTextSection);
    }
}

fn rgb_color(c: [u8; 3]) -> Color {
    Color::Rgb(Rgb {
        r: c[0] as f32 / 255.0,
        g: c[1] as f32 / 255.0,
        b: c[2] as f32 / 255.0,
        icc_profile: None,
    })
}

fn rect_op(r: render_ir::Rect, height_pt: f32, mode: PaintMode) -> Op {
    let x = pt(r.x);
    let y = height_pt - pt(r.y) - pt(r.height);
    Op::DrawPolygon {
        polygon: Polygon {
            rings: vec![PolygonRing {
                points: vec![
                    point(x, y),
                    point(x + pt(r.width), y),
                    point(x + pt(r.width), y + pt(r.height)),
                    point(x, y + pt(r.height)),
                ],
            }],
            mode,
            winding_order: WindingOrder::NonZero,
        },
    }
}

fn point(x: f32, y: f32) -> LinePoint {
    LinePoint {
        p: Point { x: Pt(x), y: Pt(y) },
        bezier: false,
    }
}
