//! Lays HTML out with blitz and extracts our own display list.
//! This is the only crate that knows blitz types — the boundary exists so that
//! swapping the layout engine does not leak into paginate, pdf-out or
//! raster-out.
//!
//! **Assumes `scale = 1.0`.** `parley` works in device px and blitz divides
//! measurements by the scale when building the layout
//! (`blitz-dom/src/layout/inline.rs`). Since the viewport is created at scale
//! 1.0, device px and CSS px coincide and the extraction can mix the two
//! coordinate sources without converting. Changing the scale requires revisiting
//! this.

use crate::net::{DataUriProvider, ResourceCollector};
use crate::style::{background_color, box_border, brush_color};
use blitz_dom::DocumentConfig;
use blitz_html::HtmlDocument;
use blitz_traits::shell::{ColorScheme, Viewport};
use render_ir::{BoxItem, DisplayList, FontResource, Glyph, ImageItem, Rect, TextRun};
use std::sync::Arc;

/// Initial layout viewport height. The document is continuous: the real height
/// comes from `DisplayList::content_height`, and page slicing is paginate's job.
const INITIAL_HEIGHT_PX: u32 = 20_000;

/// Hard cap on viewport growth. Reaching it is a design error, not a normal
/// case — hence the warning on `stderr` instead of a silent truncation.
const MAX_HEIGHT_PX: u32 = 400_000;

pub fn render_html(html: &str, width_px: f32) -> DisplayList {
    // The viewport is an integer: rounding (rather than truncating) avoids
    // losing the last column of px at widths like 595.28 (A4). The minimum of 1
    // also absorbs a negative or NaN width, which would saturate to 0.
    let viewport_width = width_px.round().max(1.0) as u32;

    // blitz only understands SVG that arrives as a resource; an `<svg>` written
    // in the HTML becomes an empty box. The rewrite normalizes that before the
    // parse.
    let html = &crate::svg::inline_svg_to_img(html);

    let mut height = INITIAL_HEIGHT_PX;
    loop {
        let (dl, used_height) = layout_and_extract(html, width_px, viewport_width, height);

        if used_height <= height as f32 {
            return dl;
        }
        if height >= MAX_HEIGHT_PX {
            eprintln!(
                "render-core: content of {used_height:.0}px exceeds the layout cap \
                 of {MAX_HEIGHT_PX}px; the excess was truncated"
            );
            return dl;
        }
        height = height.saturating_mul(2).min(MAX_HEIGHT_PX);
    }
}

/// Lays out at the requested viewport height and returns the display list plus
/// the height the content actually took (the basis for deciding to grow).
fn layout_and_extract(
    html: &str,
    width_px: f32,
    viewport_width: u32,
    viewport_height: u32,
) -> (DisplayList, f32) {
    // The provider resolves `data:` synchronously and refuses any other scheme;
    // the collector holds what was resolved so we can apply it below.
    let collector = Arc::new(ResourceCollector::default());
    let mut doc = HtmlDocument::from_html(
        html,
        DocumentConfig {
            net_provider: Some(Arc::new(DataUriProvider::new(collector.clone()))),
            ..Default::default()
        },
    );
    doc.set_viewport(Viewport::new(
        viewport_width,
        viewport_height,
        1.0,
        ColorScheme::Light,
    ));
    doc.resolve(0.0);

    // Images only exist once the resource enters the document, and that
    // invalidates the layout — hence the second resolve.
    let resources = collector.drain();
    let had_resources = !resources.is_empty();
    for resource in resources {
        doc.load_resource(resource);
    }
    if had_resources {
        doc.resolve(0.0);
    }

    let mut dl = DisplayList {
        width: width_px,
        ..Default::default()
    };
    let mut fonts: Vec<FontResource> = Vec::new();

    let tree = doc.tree();

    for (_id, node) in tree.iter() {
        // `final_layout.location` is relative to the parent; the display list is
        // in document coordinates.
        let pos = node.absolute_position(0.0, 0.0);
        let layout = &node.final_layout;
        let (x, y) = (pos.x, pos.y);

        let Some(el) = node.element_data() else {
            continue;
        };

        let node_box = Rect {
            x,
            y,
            width: layout.size.width,
            height: layout.size.height,
        };

        if let Some(s) = node.primary_styles().as_deref() {
            let background = background_color(s);
            let (border_color, border_width) = box_border(s);

            // A node with no visible background and no border paints nothing:
            // emitting a box here would fill the list with `<html>`, `<head>`
            // and `<style>`.
            if background.is_some() || border_width > 0.0 {
                dl.boxes.push(BoxItem {
                    rect: node_box,
                    background,
                    border_color,
                    border_width,
                });
            }

            // `background-image` layers paint over the background color and
            // under the element's own content, which is the order the display
            // list already has: boxes first, then images.
            dl.images
                .extend(crate::background::extract(el, s, node_box));
        }

        if let Some(image) = extract_image(el, node_box) {
            dl.images.push(image);
        }

        let Some(text_layout) = el.inline_layout_data.as_ref() else {
            continue;
        };

        // The parley layout is anchored at the content box; `absolute_position`
        // gives the border box. Same adjustment blitz makes in `Node::hit`.
        let text_x = x + layout.padding.left + layout.border.left;
        let text_y = y + layout.padding.top + layout.border.top;

        for line in text_layout.layout.lines() {
            for item in line.items() {
                let parley::PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                    continue;
                };
                let run = glyph_run.run();
                let font_index = intern_font(&mut fonts, run.font());
                let size = run.font_size();

                let offset = glyph_run.offset();
                let baseline = glyph_run.baseline();

                // `glyphs()` yields only the shaper offset, without accumulating
                // `advance` — every glyph would land on the same x.
                // `positioned_glyphs()` accumulates, but already adds the run's
                // `offset`/`baseline`; we subtract both to honour the
                // `render_ir::Glyph` contract (position relative to the run
                // origin) without counting them twice.
                let glyphs: Vec<Glyph> = glyph_run
                    .positioned_glyphs()
                    .map(|g| Glyph {
                        id: g.id as u16,
                        x: g.x - offset,
                        y: g.y - baseline,
                    })
                    .collect();
                if glyphs.is_empty() {
                    continue;
                }

                let range = run.text_range();
                let slice = text_layout.text.get(range.start..range.end);
                debug_assert!(
                    slice.is_some(),
                    "text_range {range:?} outside a char boundary of the node text; \
                     the PDF ToUnicode would silently come out empty"
                );

                dl.texts.push(TextRun {
                    origin_x: text_x + offset,
                    baseline_y: text_y + baseline,
                    font_index,
                    font_size_px: size,
                    color: brush_color(tree, glyph_run.style().brush.id),
                    glyphs,
                    text: slice.unwrap_or_default().to_string(),
                });
            }
        }
    }

    dl.fonts = fonts;

    let root = doc.root_element().final_layout;
    // The root knows the height the layout reserved; the display list only knows
    // what was painted. Keeping both lets a screenshot see deliberate whitespace
    // without pagination losing content drawn outside the root.
    dl.layout_height = root.size.height.max(root.content_size.height);
    let used_height = dl.content_height();

    (dl, used_height)
}

/// The image drawn by this element, raster or SVG, already sized to its box.
fn extract_image(el: &blitz_dom::node::ElementData, node_box: Rect) -> Option<ImageItem> {
    if let Some(raster) = el.raster_image_data() {
        return Some(ImageItem {
            rect: node_box,
            width_px: raster.width,
            height_px: raster.height,
            rgba: raster.data.clone(),
        });
    }

    // SVG is vector art, but the rest of the pipeline (paginate, pdf-out,
    // raster-out) only knows bitmaps: rasterize at the final box size.
    let tree = el.svg_data()?;
    let (width, height, rgba) = crate::svg::rasterize(tree, node_box.width, node_box.height)?;
    Some(ImageItem {
        rect: node_box,
        width_px: width,
        height_px: height,
        rgba: Arc::new(rgba),
    })
}

/// Stores the font once and returns its index in `DisplayList::fonts`.
fn intern_font(fonts: &mut Vec<FontResource>, font: &parley::FontData) -> usize {
    let bytes: &[u8] = font.data.as_ref();
    let face = font.index as usize;
    if let Some(pos) = fonts
        .iter()
        .position(|f| f.face_index == face && f.bytes.len() == bytes.len() && f.bytes == bytes)
    {
        return pos;
    }
    fonts.push(FontResource {
        bytes: bytes.to_vec(),
        face_index: face,
    });
    fonts.len() - 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;

    const PARAGRAPH_HTML: &str = r#"<!DOCTYPE html><html><head><style>
        body { margin: 0; font-family: Helvetica, Arial, sans-serif; font-size: 16px; }
        </style></head><body><p>Texto de teste</p></body></html>"#;

    #[test]
    fn extracts_glyphs_from_a_paragraph() {
        let dl = render_html(PARAGRAPH_HTML, 643.0);
        let total: usize = dl.texts.iter().map(|t| t.glyphs.len()).sum();
        assert!(total >= 13, "expected at least one glyph per character, got {total}");
        assert!(!dl.fonts.is_empty(), "no font collected");
    }

    #[test]
    fn runs_have_a_positive_baseline_inside_the_width() {
        let dl = render_html(PARAGRAPH_HTML, 643.0);
        for run in &dl.texts {
            assert!(run.baseline_y > 0.0, "baseline not placed");
            assert!(run.origin_x >= 0.0 && run.origin_x < 643.0);
            assert!(run.font_index < dl.fonts.len(), "font_index outside fonts");
        }
    }

    #[test]
    fn longer_text_takes_more_height() {
        let short = render_html(PARAGRAPH_HTML, 300.0);
        let long_html = PARAGRAPH_HTML.replace(
            "Texto de teste",
            &"Texto de teste bem mais longo para forçar quebra em várias linhas. ".repeat(10),
        );
        let long = render_html(&long_html, 300.0);
        assert!(long.content_height() > short.content_height());
    }

    #[test]
    fn table_cells_become_positioned_boxes() {
        let html = r#"<!DOCTYPE html><html><head><style>
            body { margin: 0; }
            table { width: 100%; font-size: 10pt; }
            td { border: 1px solid #000; padding: 4px; }
            </style></head><body><table>
            <tr><td>Alpha</td><td>Beta</td></tr>
            </table></body></html>"#;
        let dl = render_html(html, 600.0);
        let with_border: Vec<_> = dl.boxes.iter().filter(|b| b.border_width > 0.0).collect();
        assert!(with_border.len() >= 2, "expected cell boxes with borders");
        // The two cells sit side by side, not stacked.
        let xs: Vec<f32> = with_border.iter().map(|b| b.rect.x).collect();
        assert!(xs.iter().any(|x| *x > 0.0), "cells were not laid out in columns");
    }

    #[test]
    fn display_list_width_is_the_requested_width() {
        let dl = render_html(PARAGRAPH_HTML, 500.0);
        assert_eq!(dl.width, 500.0);
    }

    #[test]
    fn glyphs_in_a_run_advance_horizontally() {
        let dl = render_html(PARAGRAPH_HTML, 643.0);
        assert!(!dl.texts.is_empty(), "no run extracted");
        let run = dl
            .texts
            .iter()
            .find(|r| r.glyphs.len() >= 2)
            .expect("expected a run with at least two glyphs");
        for pair in run.glyphs.windows(2) {
            assert!(
                pair[1].x > pair[0].x,
                "glyphs do not advance: {:?} after {:?}",
                pair[1],
                pair[0]
            );
        }
    }

    #[test]
    fn positions_are_absolute_in_the_document() {
        let html = r#"<!DOCTYPE html><html><head><style>
            body { margin: 0; font-size: 16px; }
            </style></head><body>
            <div style="margin-top:500px;background:#ff0000">Empurrado</div>
            </body></html>"#;
        let dl = render_html(html, 600.0);

        let box_item = dl
            .boxes
            .iter()
            .find(|b| b.background == Some([255, 0, 0]))
            .expect("expected the red box");
        assert!(box_item.rect.y >= 495.0, "rect.y={} is not absolute", box_item.rect.y);

        assert!(!dl.texts.is_empty(), "no run extracted");
        for run in &dl.texts {
            assert!(
                run.baseline_y >= 495.0,
                "baseline_y={} is not absolute",
                run.baseline_y
            );
        }
    }

    #[test]
    fn html_without_a_declared_background_emits_no_box() {
        let dl = render_html(PARAGRAPH_HTML, 643.0);
        assert!(
            dl.boxes.is_empty(),
            "transparent background became a box: {:?}",
            dl.boxes
        );
        assert!(!dl.texts.is_empty(), "no run extracted");
    }

    #[test]
    fn head_and_style_stay_out_even_with_a_body_background() {
        let html = r#"<!DOCTYPE html><html><head><style>
            body { margin: 0; background: #ffffff; }
            </style></head><body><p>Oi</p></body></html>"#;
        let dl = render_html(html, 600.0);
        assert_eq!(
            dl.boxes.len(),
            1,
            "only the body should paint, got {:?}",
            dl.boxes
        );
        assert_eq!(dl.boxes[0].background, Some([255, 255, 255]));
    }

    #[test]
    fn text_color_comes_from_the_computed_style() {
        let html = r#"<!DOCTYPE html><html><head><style>
            body { margin: 0; font-size: 16px; }
            </style></head><body>
            <p style="color:#ff0000">Vermelho</p>
            </body></html>"#;
        let dl = render_html(html, 600.0);
        assert!(!dl.texts.is_empty(), "no run extracted");
        assert!(
            dl.texts.iter().any(|r| r.color == [255, 0, 0]),
            "expected a red run, got {:?}",
            dl.texts.iter().map(|r| r.color).collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_viewport_grows_for_tall_content() {
        let html = r#"<!DOCTYPE html><html><head><style>
            body { margin: 0; }
            </style></head><body>
            <div style="height:30000px"></div>
            <div style="height:20px;background:#00ff00">fim</div>
            </body></html>"#;
        let dl = render_html(html, 600.0);
        let green = dl
            .boxes
            .iter()
            .find(|b| b.background == Some([0, 255, 0]))
            .expect("the box after 30000px vanished — the viewport did not grow");
        assert!(
            green.rect.y >= 30000.0,
            "rect.y={} — content was truncated by the initial viewport",
            green.rect.y
        );
        assert!(dl.content_height() > 20_000.0);
    }

    /// 4x2 red PNG, inlined so the test needs no binary fixture.
    const PNG_4X2: &str = "iVBORw0KGgoAAAANSUhEUgAAAAQAAAACCAIAAADwyuo0AAAAEElEQVR4nGM4IScHRwzIHABvCgghBqXSdgAAAABJRU5ErkJggg==";

    #[test]
    fn a_data_uri_image_becomes_a_display_list_item() {
        let html = format!(
            r#"<img style="width:80px;height:40px" src="data:image/png;base64,{PNG_4X2}" />"#
        );
        let dl = render_html(&html, 500.0);
        assert_eq!(dl.images.len(), 1, "the image did not reach the display list");
        let img = &dl.images[0];
        assert_eq!((img.width_px, img.height_px), (4, 2), "bitmap size");
        assert!((img.rect.width - 80.0).abs() < 1.0, "box width: {:?}", img.rect);
        assert!((img.rect.height - 40.0).abs() < 1.0, "box height: {:?}", img.rect);
        assert_eq!(img.rgba.len(), 4 * 2 * 4, "RGBA8 of 4x2");
    }

    #[test]
    fn an_image_without_explicit_size_uses_its_intrinsic_size() {
        let html = format!(r#"<img src="data:image/png;base64,{PNG_4X2}" />"#);
        let dl = render_html(&html, 500.0);
        let img = &dl.images[0];
        assert!((img.rect.width - 4.0).abs() < 1.0, "intrinsic width: {:?}", img.rect);
        assert!((img.rect.height - 2.0).abs() < 1.0, "intrinsic height: {:?}", img.rect);
    }

    const SVG_20X10: &str = r##"<svg width="20" height="10" xmlns="http://www.w3.org/2000/svg"><rect width="20" height="10" fill="#0000ff"/></svg>"##;

    #[test]
    fn svg_in_an_img_data_uri_becomes_a_display_list_item() {
        let b64 = base64::engine::general_purpose::STANDARD.encode(SVG_20X10);
        let html = format!(r#"<img style="width:40px;height:20px" src="data:image/svg+xml;base64,{b64}" />"#);
        let dl = render_html(&html, 500.0);
        assert_eq!(dl.images.len(), 1, "the SVG did not reach the display list");
        let img = &dl.images[0];
        assert!((img.rect.width - 40.0).abs() < 1.0, "box: {:?}", img.rect);
        // Rasterized at 3x the box, not at the viewBox intrinsic size.
        assert_eq!((img.width_px, img.height_px), (120, 60));
        assert_eq!(img.rgba.len(), 120 * 60 * 4);
    }

    #[test]
    fn svg_without_a_size_uses_its_intrinsic_size() {
        let b64 = base64::engine::general_purpose::STANDARD.encode(SVG_20X10);
        let html = format!(r#"<img src="data:image/svg+xml;base64,{b64}" />"#);
        let dl = render_html(&html, 500.0);
        let img = &dl.images[0];
        assert!((img.rect.width - 20.0).abs() < 1.0, "intrinsic width: {:?}", img.rect);
        assert!((img.rect.height - 10.0).abs() < 1.0, "intrinsic height: {:?}", img.rect);
    }

    #[test]
    fn inline_svg_becomes_an_image_with_its_intrinsic_size() {
        let dl = render_html(&format!("<body style=\"margin:0\">{SVG_20X10}</body>"), 500.0);
        assert_eq!(dl.images.len(), 1, "the inline <svg> did not become an image");
        let img = &dl.images[0];
        assert!((img.rect.width - 20.0).abs() < 1.0, "box: {:?}", img.rect);
        assert!((img.rect.height - 10.0).abs() < 1.0, "box: {:?}", img.rect);
    }

    #[test]
    fn inline_svg_honours_the_css_on_its_tag() {
        let styled = SVG_20X10.replace("<svg ", r#"<svg style="width:100px;height:50px" "#);
        let dl = render_html(&format!("<body style=\"margin:0\">{styled}</body>"), 500.0);
        let img = &dl.images[0];
        assert!((img.rect.width - 100.0).abs() < 1.0, "box: {:?}", img.rect);
        assert!((img.rect.height - 50.0).abs() < 1.0, "box: {:?}", img.rect);
    }

    // Security invariant: only data: is resolved.

    #[test]
    fn an_http_image_is_ignored_without_opening_a_socket() {
        let dl = render_html(r#"<img src="https://exemplo.invalido/logo.png" />"#, 500.0);
        assert!(dl.images.is_empty(), "a non-data: scheme must not become an image");
    }

    #[test]
    fn an_http_svg_is_ignored_without_opening_a_socket() {
        let dl = render_html(r#"<img src="https://exemplo.invalido/logo.svg" />"#, 500.0);
        assert!(dl.images.is_empty(), "a non-data: scheme must not become an image");
    }
}
