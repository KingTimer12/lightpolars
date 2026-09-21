//! Rasterizes a parsed SVG tree into the RGBA bitmap the rest of the pipeline
//! consumes. `paginate` and `pdf-out` only know bitmaps, so the vector form
//! stops here.

/// Samples per CSS px when rasterizing. SVG is vector art and the PDF comes out
/// at 72dpi (1px = 1pt); rasterizing 1:1 would leave a logo visibly jagged in
/// print.
const SUPERSAMPLE: f32 = 3.0;

/// Cap on each side of the generated bitmap. An `<svg>` covering a whole A4
/// page at 3x is around 1800x2500; the limit keeps an absurdly large SVG from
/// turning into hundreds of MB of RGBA.
const MAX_SIDE_PX: u32 = 4096;

/// Rasterizes the tree at the given box size (in CSS px), returning
/// `(width, height, straight RGBA8)`.
///
/// `tiny-skia` works in premultiplied RGBA; `printpdf` expects straight
/// components (it splits the alpha into an `/SMask`). Without the `demultiply`
/// the antialiased edges of the SVG would come out darkened.
pub fn rasterize(tree: &usvg::Tree, css_width: f32, css_height: f32) -> Option<(u32, u32, Vec<u8>)> {
    if !css_width.is_finite() || !css_height.is_finite() || css_width <= 0.0 || css_height <= 0.0 {
        return None;
    }

    let scale = scale_within_limit(css_width, css_height);
    let width = (css_width * scale).round().max(1.0) as u32;
    let height = (css_height * scale).round().max(1.0) as u32;

    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)?;

    // The tree has its own size (the viewBox); the transform maps that size
    // onto the bitmap, which is the layout box times the supersampling factor.
    let size = tree.size();
    if size.width() <= 0.0 || size.height() <= 0.0 {
        return None;
    }
    let transform = resvg::tiny_skia::Transform::from_scale(
        width as f32 / size.width(),
        height as f32 / size.height(),
    );
    resvg::render(tree, transform, &mut pixmap.as_mut());

    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for px in pixmap.pixels() {
        let c = px.demultiply();
        rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }

    Some((width, height, rgba))
}

/// The requested supersampling, lowered if it would exceed `MAX_SIDE_PX`.
fn scale_within_limit(css_width: f32, css_height: f32) -> f32 {
    let longest = css_width.max(css_height);
    SUPERSAMPLE.min(MAX_SIDE_PX as f32 / longest)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SVG: &str = r##"<svg width="20" height="10" xmlns="http://www.w3.org/2000/svg"><rect width="20" height="10" fill="#f00"/></svg>"##;

    fn tree(source: &str) -> usvg::Tree {
        usvg::Tree::from_data(source.as_bytes(), &usvg::Options::default()).unwrap()
    }

    #[test]
    fn rasterizes_with_supersampling() {
        let (w, h, rgba) = rasterize(&tree(SVG), 20.0, 10.0).unwrap();
        assert_eq!((w, h), (60, 30), "expected 3x the box size");
        assert_eq!(rgba.len() as u32, w * h * 4);
        // The middle pixel is the rect's red, fully opaque.
        let middle = ((h / 2 * w + w / 2) * 4) as usize;
        assert_eq!(&rgba[middle..middle + 4], &[255, 0, 0, 255]);
    }

    #[test]
    fn transparent_area_comes_out_with_zero_alpha() {
        let source = r##"<svg width="10" height="10" xmlns="http://www.w3.org/2000/svg"><rect x="0" y="0" width="2" height="2" fill="#00f"/></svg>"##;
        let (w, h, rgba) = rasterize(&tree(source), 10.0, 10.0).unwrap();
        let corner = ((h - 1) * w + (w - 1)) as usize * 4;
        assert_eq!(rgba[corner + 3], 0, "the empty corner should be transparent");
    }

    #[test]
    fn a_huge_box_respects_the_cap() {
        let (w, h, _) = rasterize(&tree(SVG), 8000.0, 4000.0).unwrap();
        // 8000px of box at 3x would be 24000: the scale drops to fit 4096, and
        // the aspect ratio is preserved.
        assert_eq!((w, h), (MAX_SIDE_PX, MAX_SIDE_PX / 2), "{w}x{h}");
    }

    #[test]
    fn a_degenerate_box_does_not_panic() {
        assert!(rasterize(&tree(SVG), 0.0, 10.0).is_none());
        assert!(rasterize(&tree(SVG), f32::NAN, 10.0).is_none());
        assert!(rasterize(&tree(SVG), -5.0, 10.0).is_none());
    }
}
