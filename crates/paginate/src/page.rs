//! A single laid-out page and the one primitive used to fill it.

use render_ir::{BoxItem, DisplayList, ImageItem, TextRun};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Page {
    pub boxes: Vec<BoxItem>,
    pub texts: Vec<TextRun>,
    pub images: Vec<ImageItem>,
}

impl Page {
    /// Copies every item of `source` onto the page, shifted by `(dx, dy)` and
    /// with text runs reindexed into the merged font table.
    ///
    /// Images go first so a header background never paints over its own text.
    pub(crate) fn append_shifted(
        &mut self,
        source: &DisplayList,
        dx: f32,
        dy: f32,
        font_offset: usize,
    ) {
        for i in &source.images {
            let mut item = i.clone();
            item.rect.x += dx;
            item.rect.y += dy;
            self.images.push(item);
        }
        for b in &source.boxes {
            let mut item = b.clone();
            item.rect.x += dx;
            item.rect.y += dy;
            self.boxes.push(item);
        }
        for t in &source.texts {
            let mut item = t.clone();
            item.origin_x += dx;
            item.baseline_y += dy;
            item.font_index += font_offset;
            self.texts.push(item);
        }
    }
}
