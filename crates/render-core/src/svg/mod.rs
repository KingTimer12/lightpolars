//! SVG support: rasterizing usvg trees and rewriting inline `<svg>` elements.
//!
//! Two separate concerns, one per module:
//! - `raster`: turns a parsed `usvg::Tree` into RGBA pixels.
//! - `inline`: normalizes `<svg>` written directly in the HTML into an `<img>`,
//!   because blitz-dom only understands SVG that arrives as a resource.

mod inline;
mod raster;

pub use inline::inline_svg_to_img;
pub use raster::rasterize;
