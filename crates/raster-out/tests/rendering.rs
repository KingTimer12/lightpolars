//! Pixel-level checks of the drawing, through the public API.

use paginate::{Margins, Page, PageGeometry};
use raster_out::{MAX_SIDE_PX, render_jpeg, render_png, render_pixmap};
use render_ir::{BoxItem, Glyph, ImageItem, Rect, TextRun};
use std::sync::Arc;
use tiny_skia::Pixmap;

fn geo(w: f32, h: f32) -> PageGeometry {
    PageGeometry {
        sheet_width: w,
        sheet_height: h,
        margins: Margins::default(),
    }
}

fn pixel(p: &Pixmap, x: u32, y: u32) -> [u8; 4] {
    let px = p.pixel(x, y).unwrap().demultiply();
    [px.red(), px.green(), px.blue(), px.alpha()]
}

fn box_page(rect: Rect, background: [u8; 3]) -> Page {
    Page {
        boxes: vec![BoxItem {
            rect,
            background: Some(background),
            border_color: None,
            border_width: 0.0,
        }],
        ..Default::default()
    }
}

fn image_page(rect: Rect, width_px: u32, height_px: u32, rgba: Vec<u8>) -> Page {
    Page {
        images: vec![ImageItem {
            rect,
            width_px,
            height_px,
            rgba: Arc::new(rgba),
        }],
        ..Default::default()
    }
}

#[test]
fn an_empty_page_comes_out_white_at_the_sheet_size() {
    let p = render_pixmap(&Page::default(), &[], &geo(100.0, 50.0), 1.0).unwrap();
    assert_eq!((p.width(), p.height()), (100, 50));
    assert_eq!(pixel(&p, 50, 25), [255, 255, 255, 255]);
}

#[test]
fn the_scale_multiplies_the_dimensions() {
    let p = render_pixmap(&Page::default(), &[], &geo(100.0, 50.0), 2.0).unwrap();
    assert_eq!((p.width(), p.height()), (200, 100));
}

#[test]
fn a_box_with_a_background_paints_in_the_right_place() {
    let page = box_page(Rect { x: 10.0, y: 10.0, width: 20.0, height: 20.0 }, [255, 0, 0]);
    let p = render_pixmap(&page, &[], &geo(100.0, 100.0), 1.0).unwrap();
    assert_eq!(pixel(&p, 20, 20), [255, 0, 0, 255], "inside the box");
    assert_eq!(pixel(&p, 5, 5), [255, 255, 255, 255], "outside the box");
}

#[test]
fn a_scaled_box_follows_the_factor() {
    let page = box_page(Rect { x: 10.0, y: 10.0, width: 20.0, height: 20.0 }, [0, 0, 255]);
    let p = render_pixmap(&page, &[], &geo(100.0, 100.0), 2.0).unwrap();
    assert_eq!(pixel(&p, 40, 40), [0, 0, 255, 255], "box doubled");
    assert_eq!(pixel(&p, 10, 10), [255, 255, 255, 255], "before the doubled box");
}

#[test]
fn an_image_is_stretched_to_its_box() {
    // 1x1 opaque green, drawn into a 20x20 box.
    let page = image_page(
        Rect { x: 10.0, y: 10.0, width: 20.0, height: 20.0 },
        1,
        1,
        vec![0, 255, 0, 255],
    );
    let p = render_pixmap(&page, &[], &geo(100.0, 100.0), 1.0).unwrap();
    assert_eq!(pixel(&p, 20, 20), [0, 255, 0, 255]);
    assert_eq!(pixel(&p, 5, 5), [255, 255, 255, 255]);
}

#[test]
fn a_translucent_image_blends_with_the_background() {
    // Black at 50% alpha: over white it should come out mid grey.
    let page = image_page(
        Rect { x: 0.0, y: 0.0, width: 10.0, height: 10.0 },
        1,
        1,
        vec![0, 0, 0, 128],
    );
    let p = render_pixmap(&page, &[], &geo(20.0, 20.0), 1.0).unwrap();
    let c = pixel(&p, 5, 5);
    assert!((100..=155).contains(&c[0]), "expected mid grey, got {c:?}");
    assert_eq!(c[3], 255, "the background is opaque");
}

#[test]
fn an_image_with_a_wrongly_sized_buffer_is_ignored() {
    let page = image_page(
        Rect { x: 0.0, y: 0.0, width: 10.0, height: 10.0 },
        4,
        4,
        vec![0, 0, 0, 255],
    );
    let p = render_pixmap(&page, &[], &geo(20.0, 20.0), 1.0).unwrap();
    assert_eq!(pixel(&p, 5, 5), [255, 255, 255, 255], "nothing should be painted");
}

#[test]
fn a_run_with_a_missing_font_does_not_panic() {
    let page = Page {
        texts: vec![TextRun {
            origin_x: 5.0,
            baseline_y: 20.0,
            font_index: 7,
            font_size_px: 16.0,
            color: [0, 0, 0],
            glyphs: vec![Glyph { id: 1, x: 0.0, y: 0.0 }],
            text: "a".into(),
        }],
        ..Default::default()
    };
    let p = render_pixmap(&page, &[], &geo(50.0, 50.0), 1.0).unwrap();
    assert_eq!(pixel(&p, 10, 15), [255, 255, 255, 255]);
}

#[test]
fn degenerate_geometry_returns_none() {
    assert!(render_pixmap(&Page::default(), &[], &geo(0.0, 10.0), 1.0).is_none());
    assert!(render_pixmap(&Page::default(), &[], &geo(10.0, 10.0), 0.0).is_none());
    assert!(render_pixmap(&Page::default(), &[], &geo(f32::NAN, 10.0), 1.0).is_none());
}

#[test]
fn an_absurd_scale_is_capped() {
    let p = render_pixmap(&Page::default(), &[], &geo(1000.0, 500.0), 100.0).unwrap();
    assert_eq!(p.width(), MAX_SIDE_PX);
}

#[test]
fn the_generated_png_has_a_valid_signature() {
    let bytes = render_png(&Page::default(), &[], &geo(10.0, 10.0), 1.0).unwrap();
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
}

#[test]
fn the_generated_jpeg_has_a_valid_marker() {
    let bytes = render_jpeg(&Page::default(), &[], &geo(10.0, 10.0), 1.0, 80).unwrap();
    assert_eq!(&bytes[..2], b"\xff\xd8", "JPEG SOI");
}

#[test]
fn an_out_of_range_quality_does_not_panic() {
    assert!(render_jpeg(&Page::default(), &[], &geo(10.0, 10.0), 1.0, 0).is_some());
    assert!(render_jpeg(&Page::default(), &[], &geo(10.0, 10.0), 1.0, 255).is_some());
}
