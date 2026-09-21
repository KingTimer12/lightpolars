//! Reading paint properties out of stylo's computed values.
//!
//! Kept apart from `extract` so the traversal there stays about *what* is
//! emitted, while the "how does stylo spell this property" details live here.

use style::properties::ComputedValues;

/// Background color, or `None` when there is nothing visible to paint.
///
/// The initial value of `background-color` is `transparent`, which in stylo is
/// the `Absolute` variant (black with alpha 0). Reading only the RGB components
/// would turn every element into an opaque black box.
pub fn background_color(style: &ComputedValues) -> Option<[u8; 3]> {
    let color = style.get_background().background_color.as_absolute()?;
    if color.alpha <= 0.0 {
        return None;
    }
    Some(rgb(color))
}

/// Color and width of the top border. `render_ir::BoxItem` only carries one
/// border, so asymmetric borders are represented by the top one.
pub fn box_border(style: &ComputedValues) -> (Option<[u8; 3]>, f32) {
    let border = style.get_border();
    let width = border.border_top_width.to_f32_px();
    if width <= 0.0 {
        return (None, 0.0);
    }
    // The initial value of `border-color` is `currentcolor`, which is not
    // `Absolute`: resolve it through the element's own `color` property.
    let color = border
        .border_top_color
        .as_absolute()
        .copied()
        .unwrap_or_else(|| style.get_inherited_text().clone_color());
    (Some(rgb(&color)), width)
}

/// The blitz `TextBrush` carries no color: it is the node id of the span that
/// produced the run (`blitz-dom/src/node/element.rs`). The color comes from
/// that node's computed `color` property.
pub fn brush_color(tree: &slab::Slab<blitz_dom::Node>, id: usize) -> [u8; 3] {
    tree.get(id)
        .and_then(|n| n.primary_styles())
        .map(|s| rgb(&s.get_inherited_text().clone_color()))
        .unwrap_or([0, 0, 0])
}

pub fn rgb(c: &style::color::AbsoluteColor) -> [u8; 3] {
    let srgb = c.to_color_space(style::color::ColorSpace::Srgb);
    [
        (srgb.components.0 * 255.0).round().clamp(0.0, 255.0) as u8,
        (srgb.components.1 * 255.0).round().clamp(0.0, 255.0) as u8,
        (srgb.components.2 * 255.0).round().clamp(0.0, 255.0) as u8,
    ]
}
