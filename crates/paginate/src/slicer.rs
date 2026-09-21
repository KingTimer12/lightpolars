//! The print path: cuts a continuous document into sheets with margins and a
//! header/footer band repeated on every page.

use crate::geometry::PageGeometry;
use crate::page::Page;
use render_ir::DisplayList;

pub fn paginate(
    content: &DisplayList,
    header: Option<&DisplayList>,
    footer: Option<&DisplayList>,
    geo: &PageGeometry,
) -> Vec<Page> {
    let usable_height = geo.content_height();
    if usable_height <= 0.0 {
        return vec![Page::default()];
    }

    // Offsets matched to the order of `merge_fonts`.
    let header_offset = content.fonts.len();
    let footer_offset = header_offset + header.map_or(0, |h| h.fonts.len());

    let slices = compute_slices(content, usable_height);
    let mut pages = Vec::with_capacity(slices.len());

    for slice in &slices {
        let mut page = Page::default();

        if let Some(h) = header {
            page.append_shifted(h, geo.margins.left, 0.0, header_offset);
        }
        if let Some(f) = footer {
            let baseline = geo.sheet_height - geo.margins.bottom;
            page.append_shifted(f, geo.margins.left, baseline, footer_offset);
        }

        let shift = geo.margins.top - slice.start;

        for b in &content.boxes {
            if b.rect.y >= slice.start && b.rect.y < slice.end {
                let mut item = b.clone();
                item.rect.x += geo.margins.left;
                item.rect.y += shift;
                page.boxes.push(item);
            }
        }
        for t in &content.texts {
            if t.baseline_y >= slice.start && t.baseline_y < slice.end {
                let mut item = t.clone();
                item.origin_x += geo.margins.left;
                item.baseline_y += shift;
                page.texts.push(item);
            }
        }
        for i in &content.images {
            if i.rect.y >= slice.start && i.rect.y < slice.end {
                let mut item = i.clone();
                item.rect.x += geo.margins.left;
                item.rect.y += shift;
                page.images.push(item);
            }
        }

        pages.push(page);
    }

    if pages.is_empty() {
        pages.push(Page::default());
    }
    pages
}

struct Slice {
    start: f32,
    end: f32,
}

/// Computes the bounds of every page, pulling a cut back so it does not split a
/// box. A poor man's `break-inside: avoid` — enough for the linear flow of a
/// rich-text editor.
fn compute_slices(content: &DisplayList, usable_height: f32) -> Vec<Slice> {
    let total = content.content_height();
    let mut slices = Vec::new();
    let mut start = 0.0_f32;

    while start < total || slices.is_empty() {
        let natural_end = start + usable_height;
        let end = pull_back_to_box_edge(content, start, natural_end);
        slices.push(Slice { start, end });
        if end <= start {
            break; // guard against an infinite loop
        }
        start = end;
        if slices.len() > 10_000 {
            break;
        }
    }
    slices
}

fn pull_back_to_box_edge(content: &DisplayList, start: f32, limit: f32) -> f32 {
    let mut cut = limit;
    let rects = content
        .boxes
        .iter()
        .map(|b| b.rect)
        .chain(content.images.iter().map(|i| i.rect));
    for r in rects {
        let top = r.y;
        let bottom = r.bottom();
        // The box straddles the cut: push the whole box to the next page.
        if top > start && top < cut && bottom > cut {
            cut = cut.min(top);
        }
    }
    cut
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fonts::merge_fonts;
    use crate::geometry::Margins;
    use render_ir::{BoxItem, FontResource, Glyph, ImageItem, Rect, TextRun};

    fn zero_margins() -> Margins {
        Margins { top: 0.0, right: 0.0, bottom: 0.0, left: 0.0 }
    }

    fn text_at(y: f32) -> TextRun {
        TextRun {
            origin_x: 0.0,
            baseline_y: y,
            font_index: 0,
            font_size_px: 12.0,
            color: [0, 0, 0],
            glyphs: vec![Glyph { id: 1, x: 0.0, y: 0.0 }],
            text: "x".into(),
        }
    }

    #[test]
    fn short_content_yields_a_single_page() {
        let mut dl = DisplayList::default();
        dl.texts.push(text_at(100.0));
        let geo = PageGeometry::a4(false, zero_margins());
        let pages = paginate(&dl, None, None, &geo);
        assert_eq!(pages.len(), 1);
    }

    #[test]
    fn long_content_splits_into_pages() {
        let mut dl = DisplayList::default();
        let geo = PageGeometry::a4(false, zero_margins());
        let h = geo.content_height();
        for i in 0..3 {
            dl.texts.push(text_at(i as f32 * h + 10.0));
        }
        let pages = paginate(&dl, None, None, &geo);
        assert_eq!(pages.len(), 3);
        assert_eq!(pages[0].texts.len(), 1);
        assert_eq!(pages[1].texts.len(), 1);
        assert_eq!(pages[2].texts.len(), 1);
    }

    #[test]
    fn coordinates_are_rewritten_into_page_space() {
        let mut dl = DisplayList::default();
        let geo = PageGeometry::a4(false, Margins {
            top: 100.0, right: 0.0, bottom: 0.0, left: 50.0,
        });
        let h = geo.content_height();
        dl.texts.push(text_at(h + 10.0));
        let pages = paginate(&dl, None, None, &geo);
        assert_eq!(pages.len(), 2);
        let run = &pages[1].texts[0];
        assert!((run.baseline_y - (100.0 + 10.0)).abs() < 0.001);
        assert!((run.origin_x - 50.0).abs() < 0.001);
    }

    #[test]
    fn the_header_repeats_on_every_page_inside_the_margin_band() {
        let mut dl = DisplayList::default();
        let geo = PageGeometry::a4(false, Margins {
            top: 100.0, right: 0.0, bottom: 0.0, left: 0.0,
        });
        let h = geo.content_height();
        dl.texts.push(text_at(10.0));
        dl.texts.push(text_at(h + 10.0));

        let mut header = DisplayList::default();
        header.texts.push(text_at(20.0));

        let pages = paginate(&dl, Some(&header), None, &geo);
        assert_eq!(pages.len(), 2);
        for page in &pages {
            let in_band = page.texts.iter().filter(|t| t.baseline_y < 100.0).count();
            assert_eq!(in_band, 1, "the header should appear once per page");
        }
    }

    #[test]
    fn a_cut_never_splits_a_box_in_half() {
        let mut dl = DisplayList::default();
        let geo = PageGeometry::a4(false, zero_margins());
        let h = geo.content_height();
        dl.boxes.push(BoxItem {
            rect: Rect { x: 0.0, y: h - 20.0, width: 100.0, height: 60.0 },
            background: None,
            border_color: Some([0, 0, 0]),
            border_width: 1.0,
        });
        let pages = paginate(&dl, None, None, &geo);
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].boxes.len(), 0);
        assert_eq!(pages[1].boxes.len(), 1);
    }

    fn font(marker: u8) -> FontResource {
        FontResource { bytes: vec![marker], face_index: 0 }
    }

    #[test]
    fn merge_fonts_concatenates_content_header_footer_in_that_order() {
        let mut content = DisplayList::default();
        content.fonts.push(font(1));
        let mut header = DisplayList::default();
        header.fonts.push(font(2));
        let mut footer = DisplayList::default();
        footer.fonts.push(font(3));

        let fonts = merge_fonts(&content, Some(&header), Some(&footer));
        assert_eq!(fonts.len(), 3);
        assert_eq!(fonts[0].bytes, vec![1]);
        assert_eq!(fonts[1].bytes, vec![2]);
        assert_eq!(fonts[2].bytes, vec![3]);
    }

    #[test]
    fn header_and_footer_font_indices_are_reindexed_into_the_single_table() {
        let geo = PageGeometry::a4(false, Margins {
            top: 100.0, right: 0.0, bottom: 100.0, left: 0.0,
        });
        let mut content = DisplayList::default();
        content.fonts.push(font(1));
        content.texts.push(text_at(10.0));

        let mut header = DisplayList::default();
        header.fonts.push(font(2));
        header.texts.push(text_at(20.0)); // font_index 0 in the header's own list

        let mut footer = DisplayList::default();
        footer.fonts.push(font(3));
        footer.texts.push(text_at(5.0));

        let pages = paginate(&content, Some(&header), Some(&footer), &geo);
        assert_eq!(pages.len(), 1);
        let idx: Vec<usize> = pages[0].texts.iter().map(|t| t.font_index).collect();
        // header, footer, content — the order paginate stacks them in.
        assert_eq!(idx, vec![1, 2, 0]);

        let fonts = merge_fonts(&content, Some(&header), Some(&footer));
        assert_eq!(fonts[idx[0]].bytes, vec![2], "header points at the header font");
        assert_eq!(fonts[idx[1]].bytes, vec![3], "footer points at the footer font");
        assert_eq!(fonts[idx[2]].bytes, vec![1], "content points at its own font");
    }

    fn image_at(y: f32, height: f32) -> ImageItem {
        ImageItem {
            rect: Rect { x: 0.0, y, width: 100.0, height },
            width_px: 10,
            height_px: 10,
            rgba: std::sync::Arc::new(vec![0; 10 * 10 * 4]),
        }
    }

    #[test]
    fn an_image_lands_on_the_right_page_with_margins_applied() {
        let geo = PageGeometry::a4(false, Margins {
            top: 100.0, right: 0.0, bottom: 0.0, left: 50.0,
        });
        let h = geo.content_height();
        let mut dl = DisplayList::default();
        dl.images.push(image_at(10.0, 20.0));
        dl.images.push(image_at(h + 10.0, 20.0));

        let pages = paginate(&dl, None, None, &geo);
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].images.len(), 1);
        assert_eq!(pages[1].images.len(), 1);
        assert!((pages[1].images[0].rect.y - 110.0).abs() < 0.001);
        assert!((pages[1].images[0].rect.x - 50.0).abs() < 0.001);
    }

    #[test]
    fn a_cut_never_splits_an_image_in_half() {
        let geo = PageGeometry::a4(false, zero_margins());
        let h = geo.content_height();
        let mut dl = DisplayList::default();
        dl.images.push(image_at(h - 20.0, 60.0));

        let pages = paginate(&dl, None, None, &geo);
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].images.len(), 0, "the image was split in half");
        assert_eq!(pages[1].images.len(), 1);
    }

    #[test]
    fn a_header_image_repeats_on_every_page() {
        let geo = PageGeometry::a4(false, Margins {
            top: 100.0, right: 0.0, bottom: 0.0, left: 0.0,
        });
        let h = geo.content_height();
        let mut dl = DisplayList::default();
        dl.texts.push(text_at(10.0));
        dl.texts.push(text_at(h + 10.0));

        let mut header = DisplayList::default();
        header.images.push(image_at(10.0, 50.0));

        let pages = paginate(&dl, Some(&header), None, &geo);
        assert_eq!(pages.len(), 2);
        for page in &pages {
            assert_eq!(page.images.len(), 1, "the header logo vanished on one page");
        }
    }
}
