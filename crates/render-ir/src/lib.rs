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
    /// Position of this item in document paint order. See `ImageItem::order`.
    pub order: u32,
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

/// Bytes of a font used by the document, with the face index inside the file.
#[derive(Debug, Clone, PartialEq)]
pub struct FontResource {
    pub bytes: Vec<u8>,
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
            order: 0,
        });
        assert_eq!(dl.content_height(), 500.0);
    }
}
