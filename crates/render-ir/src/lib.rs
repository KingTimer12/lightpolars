//! Pure types exchanged between render-core, paginate, pdf-out and raster-out.
//! No blitz or printpdf dependency — this is the drawing boundary.

pub fn px_from_mm(mm: f32) -> f32 { mm * 96.0 / 25.4 }
pub fn px_from_cm(cm: f32) -> f32 { cm * 96.0 / 2.54 }
pub fn px_from_pt(pt: f32) -> f32 { pt * 96.0 / 72.0 }

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn bottom(&self) -> f32 { self.y + self.height }
    pub fn right(&self) -> f32 { self.x + self.width }
}

/// A glyph already placed by shaping, in px, relative to the run origin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glyph {
    pub id: u16,
    pub x: f32,
    pub y: f32,
}

/// A sequence of glyphs sharing one font and size, on a single line.
#[derive(Debug, Clone, PartialEq)]
pub struct TextRun {
    pub origin_x: f32,
    pub baseline_y: f32,
    /// Index into `DisplayList::fonts`.
    pub font_index: usize,
    pub font_size_px: f32,
    pub color: [u8; 3],
    pub glyphs: Vec<Glyph>,
    /// Source text of the run, for the PDF ToUnicode map.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct BoxItem {
    pub rect: Rect,
    pub background: Option<[u8; 3]>,
    pub border_color: Option<[u8; 3]>,
    pub border_width: f32,
    /// `border-radius`, clockwise from the top-left corner, each corner as
    /// `[horizontal, vertical]`. CSS radii are elliptical: `border-radius:50%`
    /// on a 200x100 box is a 100x50 quarter-ellipse, not a circle.
    pub radii: [[f32; 2]; 4],
    /// Position of this item in document paint order. See `ImageItem::order`.
    pub order: u32,
}

/// One step of a box outline, in display-list coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathCmd {
    MoveTo([f32; 2]),
    LineTo([f32; 2]),
    /// Cubic bezier: two control points then the end point.
    CurveTo([f32; 2], [f32; 2], [f32; 2]),
}

/// Distance from a corner to the bezier handle that approximates a quarter
/// circle. The classic value; the error is under 0.03% of the radius.
const KAPPA: f32 = 0.552_284_8;

impl BoxItem {
    pub fn has_radius(&self) -> bool {
        self.radii.iter().any(|[x, y]| *x > 0.0 || *y > 0.0)
    }

    /// The outline as a closed path, clockwise from the top-left corner.
    ///
    /// Both renderers build their own path type from this, so the corner
    /// geometry is defined once instead of twice.
    pub fn outline(&self) -> Vec<PathCmd> {
        let (l, t) = (self.rect.x, self.rect.y);
        let (r, b) = (self.rect.right(), self.rect.bottom());
        let [tl, tr, br, bl] = self.radii;

        if !self.has_radius() {
            return vec![
                PathCmd::MoveTo([l, t]),
                PathCmd::LineTo([r, t]),
                PathCmd::LineTo([r, b]),
                PathCmd::LineTo([l, b]),
                PathCmd::LineTo([l, t]),
            ];
        }

        vec![
            PathCmd::MoveTo([l + tl[0], t]),
            PathCmd::LineTo([r - tr[0], t]),
            PathCmd::CurveTo(
                [r - tr[0] + KAPPA * tr[0], t],
                [r, t + tr[1] - KAPPA * tr[1]],
                [r, t + tr[1]],
            ),
            PathCmd::LineTo([r, b - br[1]]),
            PathCmd::CurveTo(
                [r, b - br[1] + KAPPA * br[1]],
                [r - br[0] + KAPPA * br[0], b],
                [r - br[0], b],
            ),
            PathCmd::LineTo([l + bl[0], b]),
            PathCmd::CurveTo(
                [l + bl[0] - KAPPA * bl[0], b],
                [l, b - bl[1] + KAPPA * bl[1]],
                [l, b - bl[1]],
            ),
            PathCmd::LineTo([l, t + tl[1]]),
            PathCmd::CurveTo(
                [l, t + tl[1] - KAPPA * tl[1]],
                [l + tl[0] - KAPPA * tl[0], t],
                [l + tl[0], t],
            ),
        ]
    }
}

/// A decoded image, placed by layout.
/// Holds RGBA8 because that is what blitz produces and what the PDF consumes;
/// the Arc keeps page slicing from copying the whole bitmap.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ImageItem {
    pub rect: Rect,
    /// Size of the bitmap in pixels, not of the box it is drawn into.
    pub width_px: u32,
    pub height_px: u32,
    pub rgba: std::sync::Arc<Vec<u8>>,
    /// Position of this item in document paint order.
    ///
    /// Boxes and images live in separate vectors, but they interleave on the
    /// page: a `background-image` on an ancestor is painted before a
    /// descendant's background colour. Drawing every box and then every image
    /// puts a full-page background over the whole document. Renderers merge
    /// the two lists by this number instead.
    pub order: u32,
}

/// Shared, read-only font file contents.
///
/// Type-erased on purpose: the layout engine already holds every font in an
/// `Arc` of its own, so this can borrow that allocation instead of copying a
/// file that runs from hundreds of KB to several MB. It also lets a test build
/// one straight from a `Vec<u8>`, with no engine in sight.
#[derive(Clone)]
pub struct FontBytes(std::sync::Arc<dyn AsRef<[u8]> + Send + Sync>);

impl FontBytes {
    pub fn new<T: AsRef<[u8]> + Send + Sync + 'static>(bytes: T) -> Self {
        Self(std::sync::Arc::new(bytes))
    }

    /// Takes over an `Arc` the caller already holds, without copying it.
    pub fn from_arc(bytes: std::sync::Arc<dyn AsRef<[u8]> + Send + Sync>) -> Self {
        Self(bytes)
    }
}

impl std::ops::Deref for FontBytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        (*self.0).as_ref()
    }
}

impl AsRef<[u8]> for FontBytes {
    fn as_ref(&self) -> &[u8] {
        self
    }
}

/// By content, not by address: two fonts that hold the same file are the same
/// font, whichever allocation each one came from.
impl PartialEq for FontBytes {
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}

impl std::fmt::Debug for FontBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The bytes themselves are megabytes of noise; the length is what a
        // reader of a failed assertion actually wants.
        f.debug_struct("FontBytes").field("len", &self.len()).finish()
    }
}

/// Bytes of a font used by the document, with the face index inside the file.
#[derive(Debug, Clone, PartialEq)]
pub struct FontResource {
    pub bytes: FontBytes,
    pub face_index: usize,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DisplayList {
    pub boxes: Vec<BoxItem>,
    pub texts: Vec<TextRun>,
    pub images: Vec<ImageItem>,
    pub fonts: Vec<FontResource>,
    pub width: f32,
    /// Height the layout reserved for the document root, painted or not.
    ///
    /// Needed because an invisible element (a `<div style="height:3000px">`
    /// with no background) emits no item at all: judging by painted items
    /// alone the document would look zero-height and a screenshot would come
    /// out cropped.
    pub layout_height: f32,
}

impl DisplayList {
    /// Height occupied by the content. Basis for page slicing.
    pub fn content_height(&self) -> f32 {
        let from_boxes = self.boxes.iter().map(|b| b.rect.bottom());
        let from_texts = self.texts.iter().map(|t| t.baseline_y);
        let from_images = self.images.iter().map(|i| i.rect.bottom());
        from_boxes
            .chain(from_texts)
            .chain(from_images)
            .fold(self.layout_height.max(0.0), |acc, v| acc.max(v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_bottom_is_y_plus_height() {
        let r = Rect { x: 10.0, y: 20.0, width: 100.0, height: 30.0 };
        assert_eq!(r.bottom(), 50.0);
    }

    #[test]
    fn a_box_without_radii_outlines_as_a_rectangle() {
        let b = BoxItem {
            rect: Rect { x: 0.0, y: 0.0, width: 10.0, height: 20.0 },
            ..Default::default()
        };
        assert!(!b.has_radius());
        assert_eq!(b.outline().len(), 5, "move plus four lines");
        assert!(!b.outline().iter().any(|c| matches!(c, PathCmd::CurveTo(..))));
    }

    #[test]
    fn a_rounded_box_outlines_with_one_curve_per_corner() {
        let b = BoxItem {
            rect: Rect { x: 0.0, y: 0.0, width: 100.0, height: 100.0 },
            radii: [[10.0, 10.0]; 4],
            ..Default::default()
        };
        assert!(b.has_radius());
        let curves = b.outline().iter().filter(|c| matches!(c, PathCmd::CurveTo(..))).count();
        assert_eq!(curves, 4);
    }

    #[test]
    fn the_outline_starts_and_ends_at_the_same_point() {
        let b = BoxItem {
            rect: Rect { x: 5.0, y: 7.0, width: 60.0, height: 40.0 },
            radii: [[8.0, 6.0], [4.0, 4.0], [12.0, 3.0], [0.0, 0.0]],
            ..Default::default()
        };
        let path = b.outline();
        let PathCmd::MoveTo(start) = path[0] else { panic!("no move") };
        let end = match path[path.len() - 1] {
            PathCmd::CurveTo(_, _, p) | PathCmd::LineTo(p) | PathCmd::MoveTo(p) => p,
        };
        assert_eq!(start, end, "the path does not close");
    }

    #[test]
    fn every_outline_point_stays_inside_the_box() {
        let b = BoxItem {
            rect: Rect { x: 10.0, y: 20.0, width: 100.0, height: 50.0 },
            radii: [[25.0, 25.0]; 4],
            ..Default::default()
        };
        for cmd in b.outline() {
            let points = match cmd {
                PathCmd::MoveTo(p) | PathCmd::LineTo(p) => vec![p],
                PathCmd::CurveTo(a, b2, c) => vec![a, b2, c],
            };
            for [x, y] in points {
                assert!((10.0..=110.0).contains(&x), "x out of the box: {x}");
                assert!((20.0..=70.0).contains(&y), "y out of the box: {y}");
            }
        }
    }

    #[test]
    fn unit_conversions_to_px_at_96dpi() {
        assert!((px_from_mm(25.4) - 96.0).abs() < 1e-6);
        assert!((px_from_cm(2.54) - 96.0).abs() < 1e-6);
        assert!((px_from_pt(72.0) - 96.0).abs() < 1e-6);
    }

    #[test]
    fn empty_display_list_has_zero_height() {
        let dl = DisplayList::default();
        assert_eq!(dl.content_height(), 0.0);
    }

    #[test]
    fn content_height_is_the_lowest_item_bottom() {
        let mut dl = DisplayList::default();
        dl.boxes.push(BoxItem {
            rect: Rect { x: 0.0, y: 0.0, width: 10.0, height: 40.0 },
            background: None,
            border_color: None,
            border_width: 0.0,
            radii: [[0.0; 2]; 4],
            order: 0,
        });
        dl.texts.push(TextRun {
            origin_x: 0.0,
            baseline_y: 120.0,
            font_index: 0,
            font_size_px: 12.0,
            color: [0, 0, 0],
            glyphs: vec![],
            text: String::new(),
        });
        assert_eq!(dl.content_height(), 120.0);
    }

    #[test]
    fn content_height_counts_image_bottoms() {
        let mut dl = DisplayList::default();
        dl.images.push(ImageItem {
            rect: Rect { x: 0.0, y: 10.0, width: 100.0, height: 150.0 },
            width_px: 200,
            height_px: 150,
            rgba: std::sync::Arc::new(vec![0; 200 * 150 * 4]),
            order: 0,
        });
        assert_eq!(dl.content_height(), 160.0);
    }

    #[test]
    fn layout_height_counts_even_with_nothing_painted() {
        let dl = DisplayList { layout_height: 3000.0, ..Default::default() };
        assert_eq!(dl.content_height(), 3000.0);
    }

    #[test]
    fn an_item_below_the_layout_box_still_wins() {
        let mut dl = DisplayList { layout_height: 100.0, ..Default::default() };
        dl.boxes.push(BoxItem {
            rect: Rect { x: 0.0, y: 0.0, width: 10.0, height: 500.0 },
            background: None,
            border_color: None,
            border_width: 0.0,
            radii: [[0.0; 2]; 4],
            order: 0,
        });
        assert_eq!(dl.content_height(), 500.0);
    }
}
