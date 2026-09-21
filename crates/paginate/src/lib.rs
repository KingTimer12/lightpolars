//! Slicing a continuous document into pages.
//! Pure arithmetic over rectangles: renders nothing and knows nothing of blitz.
//!
//! Two consumers, two entry points:
//! - `paginate`: the print path — a sheet, margins, repeated header/footer.
//! - `capture`: the screenshot path — the whole document, or a clip of it, with
//!   no sheet and no margins.

mod capture;
mod fonts;
mod geometry;
mod page;
mod slicer;

pub use capture::{clip, whole_document};
pub use fonts::merge_fonts;
pub use geometry::{A4_HEIGHT_PX, A4_WIDTH_PX, Margins, PageGeometry};
pub use page::Page;
pub use slicer::paginate;
