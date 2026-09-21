//! The screenshot path: the whole document, or a clip of it, as one page.
//! No sheet, no margins and no slicing — the consumer wants the page as it is.

use crate::geometry::{Margins, PageGeometry};
use crate::page::Page;
use render_ir::{DisplayList, Rect};

/// Puts the whole document into a single page sized exactly to its content.
pub fn whole_document(content: &DisplayList) -> (Page, PageGeometry) {
    clip(
        content,
        Rect {
            x: 0.0,
            y: 0.0,
            width: content.width,
            height: content.content_height(),
        },
    )
}

/// A rectangular clip of the document, in the same shape as `whole_document`.
///
/// This is the `clip` of `Page.captureScreenshot`. Items are translated to the
/// clip origin; nothing is discarded, because whoever draws already ignores
/// what falls outside the bitmap — filtering here would cut by box rather than
/// by pixel, and would drop text whose baseline is outside while its glyphs are
/// inside.
pub fn clip(content: &DisplayList, area: Rect) -> (Page, PageGeometry) {
    let geo = PageGeometry {
        sheet_width: area.width.max(1.0),
        sheet_height: area.height.max(1.0),
        margins: Margins::default(),
    };
    let mut page = Page::default();
    page.append_shifted(content, -area.x, -area.y, 0);
    (page, geo)
}

#[cfg(test)]
mod tests {
    use super::*;
    use render_ir::BoxItem;

    fn dl_with_box(y: f32, height: f32) -> DisplayList {
        DisplayList {
            width: 800.0,
            boxes: vec![BoxItem {
                rect: Rect { x: 0.0, y, width: 100.0, height },
                background: Some([1, 2, 3]),
                border_color: None,
                border_width: 0.0,
                radii: [[0.0; 2]; 4],
                order: 0,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn the_sheet_takes_the_list_width_and_the_content_height() {
        let (_, geo) = whole_document(&dl_with_box(50.0, 120.0));
        assert_eq!(geo.sheet_width, 800.0);
        assert_eq!(geo.sheet_height, 170.0);
        assert_eq!(geo.margins, Margins::default());
    }

    #[test]
    fn tall_content_is_not_cut() {
        let (page, geo) = whole_document(&dl_with_box(0.0, 9000.0));
        assert_eq!(page.boxes.len(), 1);
        assert_eq!(geo.sheet_height, 9000.0);
    }

    #[test]
    fn items_keep_their_original_position() {
        let (page, _) = whole_document(&dl_with_box(50.0, 120.0));
        assert_eq!(page.boxes[0].rect.y, 50.0);
    }

    #[test]
    fn an_empty_list_yields_a_minimal_sheet_instead_of_zero() {
        let (page, geo) = whole_document(&DisplayList::default());
        assert!(page.boxes.is_empty());
        assert_eq!((geo.sheet_width, geo.sheet_height), (1.0, 1.0));
    }

    #[test]
    fn a_clip_translates_items_to_the_origin() {
        let dl = dl_with_box(300.0, 50.0);
        let (page, geo) = clip(&dl, Rect { x: 10.0, y: 280.0, width: 200.0, height: 100.0 });
        assert_eq!((geo.sheet_width, geo.sheet_height), (200.0, 100.0));
        assert_eq!(page.boxes[0].rect.x, -10.0);
        assert_eq!(page.boxes[0].rect.y, 20.0);
    }

    #[test]
    fn a_clip_does_not_discard_items_outside_its_box() {
        // Whoever draws cuts at the pixel; filtering here would lose a glyph
        // whose baseline sits outside the clip but whose ink is inside.
        let dl = dl_with_box(5000.0, 10.0);
        let (page, _) = clip(&dl, Rect { x: 0.0, y: 0.0, width: 100.0, height: 100.0 });
        assert_eq!(page.boxes.len(), 1);
    }
}
