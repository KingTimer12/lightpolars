//! CSS `background-image` layers.
//!
//! An `<img>` reaches the display list through the element's own image data;
//! a background is a paint property of any box, stored by blitz on the side
//! (`ElementData::background_images`). Without this module a
//! `background-image: url(data:…)` lays out fine and paints nothing.
//!
//! Everything is resolved here, in CSS px, and comes out as plain
//! `ImageItem`s that exactly fill their rect. That keeps the renderers free of
//! clipping: a tile that would spill past the box is cropped in pixels before
//! it is emitted, so `pdf-out` and `raster-out` need no clip path.

use blitz_dom::node::{BackgroundImageData, ElementData, ImageData};
use render_ir::{ImageItem, Rect};
use std::sync::Arc;
use style::properties::ComputedValues;
use style::values::computed::background::BackgroundSize;
use style::values::computed::length::Length;
use style::values::generics::NonNegative;
use style::values::specified::background::BackgroundRepeatKeyword;

/// Ceiling on tiles emitted per layer. A 1px image repeated over a tall page
/// would otherwise produce hundreds of thousands of items; past this point the
/// layer is left unpainted rather than allowed to blow up the display list.
///
/// Sized so that an ordinary texture still tiles a full A4 page: an 8x8 tile
/// over 595x842px is a little under 8000 tiles. The tiles share one `Arc`, so
/// what they cost is display-list entries, not pixels.
const MAX_TILES: i64 = 16_384;

/// The image items for every background layer of one element, already in paint
/// order.
pub fn extract(el: &ElementData, style: &ComputedValues, node_box: Rect) -> Vec<ImageItem> {
    if node_box.width <= 0.0 || node_box.height <= 0.0 || el.background_images.is_empty() {
        return Vec::new();
    }

    let mut items = Vec::new();
    // CSS paints the first layer on top; the display list paints in order, so
    // the layers are walked from the last one to the first.
    for (index, layer) in el.background_images.iter().enumerate().rev() {
        let Some(layer) = layer.as_ref() else { continue };
        extract_layer(layer, style, index, node_box, &mut items);
    }
    items
}

fn extract_layer(
    layer: &BackgroundImageData,
    style: &ComputedValues,
    index: usize,
    area: Rect,
    out: &mut Vec<ImageItem>,
) {
    let Some((intrinsic_width, intrinsic_height)) = intrinsic_size(&layer.image) else {
        return;
    };

    let (tile_width, tile_height) = tile_size(style, index, area, intrinsic_width, intrinsic_height);
    if tile_width <= 0.0 || tile_height <= 0.0 {
        return;
    }

    let background = style.get_background();
    let repeat = list_item(&background.background_repeat.0, index)
        .cloned()
        .unwrap_or(style::values::specified::background::BackgroundRepeat(
            BackgroundRepeatKeyword::Repeat,
            BackgroundRepeatKeyword::Repeat,
        ));

    let origin_x = area.x
        + list_item(&background.background_position_x.0, index)
            .map(|p| p.resolve(Length::new(area.width - tile_width)).px())
            .unwrap_or(0.0);
    let origin_y = area.y
        + list_item(&background.background_position_y.0, index)
            .map(|p| p.resolve(Length::new(area.height - tile_height)).px())
            .unwrap_or(0.0);

    let columns = tile_range(repeat.0, origin_x, tile_width, area.x, area.right());
    let rows = tile_range(repeat.1, origin_y, tile_height, area.y, area.bottom());
    let tiles = (columns.end - columns.start).saturating_mul(rows.end - rows.start);
    if tiles > MAX_TILES {
        return;
    }

    // Rasterized once per layer at the tile size, then reused by every tile.
    let Some(source) = pixels(&layer.image, tile_width, tile_height) else {
        return;
    };

    for row in rows.clone() {
        for column in columns.clone() {
            let tile = Rect {
                x: origin_x + column as f32 * tile_width,
                y: origin_y + row as f32 * tile_height,
                width: tile_width,
                height: tile_height,
            };
            if let Some(item) = crop_to(&source, tile, area) {
                out.push(item);
            }
        }
    }
}

/// Which tile indices are needed on one axis to cover the area.
///
/// `no-repeat` is the single tile at the origin; the repeating keywords walk
/// outwards from it until the area is covered. `space` and `round` are treated
/// as plain repetition: getting them right needs a different tile size, and
/// neither shows up in the documents this renders.
fn tile_range(
    repeat: BackgroundRepeatKeyword,
    origin: f32,
    tile: f32,
    start: f32,
    end: f32,
) -> std::ops::Range<i64> {
    if matches!(repeat, BackgroundRepeatKeyword::NoRepeat) || tile <= 0.0 {
        return 0..1;
    }
    let first = ((start - origin) / tile).floor() as i64;
    let last = ((end - origin) / tile).ceil() as i64;
    first..last.max(first + 1)
}

/// The size one tile occupies, per `background-size`.
fn tile_size(
    style: &ComputedValues,
    index: usize,
    area: Rect,
    intrinsic_width: f32,
    intrinsic_height: f32,
) -> (f32, f32) {
    let ratio = intrinsic_width / intrinsic_height;
    let size = list_item(&style.get_background().background_size.0, index)
        .cloned()
        .unwrap_or(BackgroundSize::auto());

    match size {
        BackgroundSize::Cover | BackgroundSize::Contain => {
            let scale_x = area.width / intrinsic_width;
            let scale_y = area.height / intrinsic_height;
            let scale = if matches!(size, BackgroundSize::Cover) {
                scale_x.max(scale_y)
            } else {
                scale_x.min(scale_y)
            };
            (intrinsic_width * scale, intrinsic_height * scale)
        }
        BackgroundSize::ExplicitSize { width, height } => {
            let given_width = explicit(&width, area.width);
            let given_height = explicit(&height, area.height);
            match (given_width, given_height) {
                (Some(w), Some(h)) => (w, h),
                // One axis given: the other follows the intrinsic ratio.
                (Some(w), None) => (w, w / ratio),
                (None, Some(h)) => (h * ratio, h),
                (None, None) => (intrinsic_width, intrinsic_height),
            }
        }
    }
}

/// An explicit `background-size` component, or `None` for `auto`.
fn explicit(
    value: &style::values::generics::length::GenericLengthPercentageOrAuto<
        NonNegative<style::values::computed::LengthPercentage>,
    >,
    basis: f32,
) -> Option<f32> {
    match value {
        style::values::generics::length::GenericLengthPercentageOrAuto::Auto => None,
        style::values::generics::length::GenericLengthPercentageOrAuto::LengthPercentage(lp) => {
            Some(lp.0.resolve(Length::new(basis)).px())
        }
    }
}

fn intrinsic_size(image: &ImageData) -> Option<(f32, f32)> {
    let (width, height) = match image {
        ImageData::Raster(raster) => (raster.width as f32, raster.height as f32),
        ImageData::Svg(tree) => (tree.size().width(), tree.size().height()),
        ImageData::None => return None,
    };
    (width > 0.0 && height > 0.0).then_some((width, height))
}

/// One tile's pixels. An SVG is rasterized at the tile size so it stays sharp
/// instead of being scaled up from its intrinsic box.
fn pixels(image: &ImageData, tile_width: f32, tile_height: f32) -> Option<Source> {
    match image {
        ImageData::Raster(raster) => Some(Source {
            width: raster.width,
            height: raster.height,
            rgba: raster.data.clone(),
        }),
        ImageData::Svg(tree) => {
            let (width, height, rgba) = crate::svg::rasterize(tree, tile_width, tile_height)?;
            Some(Source {
                width,
                height,
                rgba: Arc::new(rgba),
            })
        }
        ImageData::None => None,
    }
}

struct Source {
    width: u32,
    height: u32,
    rgba: Arc<Vec<u8>>,
}

/// Clips a tile to the element box, cropping the pixels rather than asking the
/// renderer for a clip path. Returns `None` when nothing of the tile is
/// visible.
fn crop_to(source: &Source, tile: Rect, area: Rect) -> Option<ImageItem> {
    let expected = source.width as usize * source.height as usize * 4;
    if source.rgba.len() != expected || expected == 0 {
        return None;
    }

    let visible = Rect {
        x: tile.x.max(area.x),
        y: tile.y.max(area.y),
        width: 0.0,
        height: 0.0,
    };
    let right = tile.right().min(area.right());
    let bottom = tile.bottom().min(area.bottom());
    let visible = Rect {
        width: right - visible.x,
        height: bottom - visible.y,
        ..visible
    };
    if visible.width <= 0.0 || visible.height <= 0.0 {
        return None;
    }

    // Whole tile inside the box: the common case, and no copy is needed.
    if visible.width >= tile.width && visible.height >= tile.height {
        return Some(ImageItem {
            rect: tile,
            width_px: source.width,
            height_px: source.height,
            rgba: source.rgba.clone(),
        });
    }

    // Map the visible rect back to source pixels. The tile scales uniformly,
    // so the mapping is linear on each axis.
    let to_px = |offset: f32, tile_side: f32, source_side: u32| -> f32 {
        offset / tile_side * source_side as f32
    };
    let left = to_px(visible.x - tile.x, tile.width, source.width).floor().max(0.0) as u32;
    let top = to_px(visible.y - tile.y, tile.height, source.height).floor().max(0.0) as u32;
    let crop_width =
        (to_px(visible.width, tile.width, source.width).ceil() as u32).min(source.width - left);
    let crop_height =
        (to_px(visible.height, tile.height, source.height).ceil() as u32).min(source.height - top);
    if crop_width == 0 || crop_height == 0 {
        return None;
    }

    let mut rgba = Vec::with_capacity(crop_width as usize * crop_height as usize * 4);
    for row in 0..crop_height {
        let start = (((top + row) * source.width + left) * 4) as usize;
        let end = start + crop_width as usize * 4;
        rgba.extend_from_slice(&source.rgba[start..end]);
    }

    // The destination rect is the source crop mapped forward again, so the
    // pixels keep the scale they would have had unclipped, and then clamped
    // back into the box. The clamp matters because the crop is in whole source
    // pixels: a 2px-wide image stretched over a 30px tile cannot express a
    // 10px cut, and rounding up would push the tile past the border.
    let scale_x = tile.width / source.width as f32;
    let scale_y = tile.height / source.height as f32;
    let x = (tile.x + left as f32 * scale_x).max(area.x);
    let y = (tile.y + top as f32 * scale_y).max(area.y);
    let right = (tile.x + (left + crop_width) as f32 * scale_x).min(area.right());
    let bottom = (tile.y + (top + crop_height) as f32 * scale_y).min(area.bottom());
    if right <= x || bottom <= y {
        return None;
    }
    Some(ImageItem {
        rect: Rect {
            x,
            y,
            width: right - x,
            height: bottom - y,
        },
        width_px: crop_width,
        height_px: crop_height,
        rgba: Arc::new(rgba),
    })
}

/// Background longhands are lists that repeat to match `background-image`.
fn list_item<T>(list: &[T], index: usize) -> Option<&T> {
    if list.is_empty() {
        return None;
    }
    list.get(index % list.len())
}

#[cfg(test)]
mod tests {
    use crate::render_html;

    /// 2x1: left pixel opaque red, right pixel opaque blue. Two pixels so a
    /// crop or a flip is visible in the output.
    const STRIPE: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAIAAAABCAYAAAD0In+KAAAADklEQVR4nGP4z8AAQv8BD/kD/YURmXYAAAAASUVORK5CYII=";

    fn page(style: &str) -> render_ir::DisplayList {
        let html = format!(
            "<!DOCTYPE html><html><head><style>body{{margin:0}}</style></head>\
             <body><div style='width:100px;height:50px;{style}'></div></body></html>"
        );
        render_html(&html, 200.0)
    }

    #[test]
    fn a_background_image_reaches_the_display_list() {
        let dl = page(&format!(
            "background-image:url({STRIPE});background-size:cover;background-repeat:no-repeat"
        ));
        assert_eq!(dl.images.len(), 1, "expected exactly one background tile");
    }

    #[test]
    fn cover_fills_the_whole_box() {
        let dl = page(&format!(
            "background-image:url({STRIPE});background-size:cover;background-repeat:no-repeat"
        ));
        let rect = dl.images[0].rect;
        // 2x1 covering 100x50 scales by 50 on the short axis, so the tile is
        // 100x50 and lands exactly on the box.
        assert!(rect.width >= 100.0, "did not cover horizontally: {rect:?}");
        assert!(rect.height >= 50.0, "did not cover vertically: {rect:?}");
    }

    #[test]
    fn contain_keeps_the_aspect_ratio_inside_the_box() {
        let dl = page(&format!(
            "background-image:url({STRIPE});background-size:contain;background-repeat:no-repeat"
        ));
        let rect = dl.images[0].rect;
        assert!(rect.width <= 100.5 && rect.height <= 50.5, "spilled: {rect:?}");
        assert!(
            (rect.width / rect.height - 2.0).abs() < 0.01,
            "ratio lost: {rect:?}"
        );
    }

    #[test]
    fn an_explicit_size_is_honoured() {
        let dl = page(&format!(
            "background-image:url({STRIPE});background-size:20px 10px;background-repeat:no-repeat"
        ));
        let rect = dl.images[0].rect;
        assert!((rect.width - 20.0).abs() < 0.01, "{rect:?}");
        assert!((rect.height - 10.0).abs() < 0.01, "{rect:?}");
    }

    #[test]
    fn repeat_tiles_across_the_box() {
        let dl = page(&format!(
            "background-image:url({STRIPE});background-size:20px 10px"
        ));
        // 100x50 covered by 20x10 tiles.
        assert_eq!(dl.images.len(), 25, "wrong tile count");
    }

    #[test]
    fn tiles_never_spill_outside_the_box() {
        let dl = page(&format!(
            "background-image:url({STRIPE});background-size:30px 30px"
        ));
        for image in &dl.images {
            assert!(image.rect.x >= -0.01, "spilled left: {:?}", image.rect);
            assert!(image.rect.y >= -0.01, "spilled up: {:?}", image.rect);
            assert!(image.rect.right() <= 100.01, "spilled right: {:?}", image.rect);
            assert!(image.rect.bottom() <= 50.01, "spilled down: {:?}", image.rect);
        }
        assert!(!dl.images.is_empty());
    }

    #[test]
    fn position_moves_the_tile() {
        let dl = page(&format!(
            "background-image:url({STRIPE});background-size:20px 10px;\
             background-repeat:no-repeat;background-position:right bottom"
        ));
        let rect = dl.images[0].rect;
        assert!((rect.right() - 100.0).abs() < 0.01, "not at the right: {rect:?}");
        assert!((rect.bottom() - 50.0).abs() < 0.01, "not at the bottom: {rect:?}");
    }

    #[test]
    fn a_background_that_is_not_a_data_uri_paints_nothing() {
        let dl = page("background-image:url(https://example.com/bg.png)");
        assert!(dl.images.is_empty(), "a network background must not resolve");
    }

    #[test]
    fn an_inline_svg_background_is_rasterized() {
        let svg = "data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHdpZHRoPSIxMCIgaGVpZ2h0PSIxMCI+PHJlY3Qgd2lkdGg9IjEwIiBoZWlnaHQ9IjEwIiBmaWxsPSJyZWQiLz48L3N2Zz4=";
        let dl = page(&format!(
            "background-image:url({svg});background-size:cover;background-repeat:no-repeat"
        ));
        assert_eq!(dl.images.len(), 1, "SVG background not painted");
        assert!(dl.images[0].width_px > 10, "not rasterized at the tile size");
    }

    #[test]
    fn a_background_color_still_paints_under_the_image() {
        let dl = page(&format!(
            "background-color:#0f0;background-image:url({STRIPE});background-repeat:no-repeat"
        ));
        assert!(
            dl.boxes.iter().any(|b| b.background == Some([0, 255, 0])),
            "the color layer disappeared"
        );
        assert_eq!(dl.images.len(), 1);
    }
}
