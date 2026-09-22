//! A single laid-out page and the one primitive used to fill it.

use render_ir::{BoxItem, DisplayList, ImageItem, TextRun};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Page {
    pub boxes: Vec<BoxItem>,
    pub texts: Vec<TextRun>,
    pub images: Vec<ImageItem>,
}

/// A box or a bitmap, as the renderer meets it while walking paint order.
#[derive(Debug, Clone, Copy)]
pub enum Painted<'a> {
    Box(&'a BoxItem),
    Image(&'a ImageItem),
}

impl Page {
    /// Boxes and images interleaved by document paint order.
    ///
    /// Drawing the two vectors one after the other is wrong whenever a
    /// `background-image` covers an area that later boxes paint into: a
    /// full-page background would land on top of every box in the document.
    /// Both vectors are already in ascending order, so this is a plain merge.
    pub fn painted(&self) -> Vec<Painted<'_>> {
        let mut out = Vec::with_capacity(self.boxes.len() + self.images.len());
        let (mut b, mut i) = (0, 0);
        while b < self.boxes.len() && i < self.images.len() {
            if self.boxes[b].order <= self.images[i].order {
                out.push(Painted::Box(&self.boxes[b]));
                b += 1;
            } else {
                out.push(Painted::Image(&self.images[i]));
                i += 1;
            }
        }
        out.extend(self.boxes[b..].iter().map(Painted::Box));
        out.extend(self.images[i..].iter().map(Painted::Image));
        out
    }

    /// Copies every item of `source` onto the page, shifted by `(dx, dy)` and
    /// with text runs reindexed into the merged font table.
    ///
    /// Each list keeps its own ascending `order`, which `painted` relies on.
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

#[cfg(test)]
mod tests {
    use super::*;
    use render_ir::Rect;

    fn boxed(order: u32) -> BoxItem {
        BoxItem { order, ..Default::default() }
    }

    fn imaged(order: u32) -> ImageItem {
        ImageItem { rect: Rect::default(), order, ..Default::default() }
    }

    fn orders(page: &Page) -> Vec<(char, u32)> {
        page.painted()
            .iter()
            .map(|p| match p {
                Painted::Box(b) => ('b', b.order),
                Painted::Image(i) => ('i', i.order),
            })
            .collect()
    }

    #[test]
    fn boxes_and_images_interleave_by_order() {
        let page = Page {
            boxes: vec![boxed(1), boxed(3)],
            images: vec![imaged(0), imaged(2)],
            ..Default::default()
        };
        assert_eq!(orders(&page), [('i', 0), ('b', 1), ('i', 2), ('b', 3)]);
    }

    #[test]
    fn a_page_with_only_boxes_keeps_them_all() {
        let page = Page { boxes: vec![boxed(0), boxed(1)], ..Default::default() };
        assert_eq!(orders(&page), [('b', 0), ('b', 1)]);
    }

    #[test]
    fn a_page_with_only_images_keeps_them_all() {
        let page = Page { images: vec![imaged(0), imaged(1)], ..Default::default() };
        assert_eq!(orders(&page), [('i', 0), ('i', 1)]);
    }

    #[test]
    fn a_tie_puts_the_box_first_because_it_is_the_element_own_background() {
        let page = Page {
            boxes: vec![boxed(5)],
            images: vec![imaged(5)],
            ..Default::default()
        };
        assert_eq!(orders(&page), [('b', 5), ('i', 5)]);
    }

    #[test]
    fn an_empty_page_paints_nothing() {
        assert!(Page::default().painted().is_empty());
    }
}
