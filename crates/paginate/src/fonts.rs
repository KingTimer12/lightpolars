//! The document-wide font table shared by every page.

use render_ir::{DisplayList, FontResource};

/// One font table for the whole document, in the same order `paginate` uses
/// when reindexing runs: content, then header, then footer.
/// `pdf_out::render_pdf` must receive exactly this list.
pub fn merge_fonts(
    content: &DisplayList,
    header: Option<&DisplayList>,
    footer: Option<&DisplayList>,
) -> Vec<FontResource> {
    let mut fonts = content.fonts.clone();
    for extra in [header, footer].into_iter().flatten() {
        fonts.extend(extra.fonts.iter().cloned());
    }
    fonts
}
