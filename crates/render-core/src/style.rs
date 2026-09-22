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

/// `border-radius` for the four corners, clockwise from the top-left, each as
/// `[horizontal, vertical]` in px.
///
/// Percentages resolve against the box: the horizontal component against the
/// width, the vertical against the height. CSS then requires that radii which
/// would overlap on a side be scaled down together, which is what keeps
/// `border-radius: 50%` a clean ellipse instead of overshooting corners.
pub fn corner_radii(style: &ComputedValues, width: f32, height: f32) -> [[f32; 2]; 4] {
    use style::values::computed::length::Length;

    let border = style.get_border();
    let resolve = |corner: &style::values::computed::BorderCornerRadius| {
        [
            corner.0.width.0.resolve(Length::new(width)).px().max(0.0),
            corner.0.height.0.resolve(Length::new(height)).px().max(0.0),
        ]
    };
    let mut radii = [
        resolve(&border.border_top_left_radius),
        resolve(&border.border_top_right_radius),
        resolve(&border.border_bottom_right_radius),
        resolve(&border.border_bottom_left_radius),
    ];

    // CSS Backgrounds 3 §5.5: f is the smallest ratio of side length to the
    // sum of the two radii meeting on that side; if any is below 1, every
    // radius shrinks by it.
    let ratio = |side: f32, a: f32, b: f32| {
        let sum = a + b;
        if sum > 0.0 { side / sum } else { f32::INFINITY }
    };
    let factor = ratio(width, radii[0][0], radii[1][0]) // top
        .min(ratio(height, radii[1][1], radii[2][1])) // right
        .min(ratio(width, radii[3][0], radii[2][0])) // bottom
        .min(ratio(height, radii[0][1], radii[3][1])); // left

    if factor < 1.0 {
        for corner in &mut radii {
            corner[0] *= factor;
            corner[1] *= factor;
        }
    }
    radii
}

#[cfg(test)]
mod tests {
    use crate::render_html;

    fn radii(style: &str) -> [[f32; 2]; 4] {
        let html = format!(
            "<!DOCTYPE html><html><head><style>body{{margin:0}}</style></head>\
             <body><div style='background:#f00;{style}'></div></body></html>"
        );
        let dl = render_html(&html, 400.0);
        dl.boxes
            .iter()
            .find(|b| b.background == Some([255, 0, 0]))
            .expect("the box was not emitted")
            .radii
    }

    #[test]
    fn a_box_without_a_radius_has_none() {
        assert_eq!(radii("width:100px;height:50px"), [[0.0; 2]; 4]);
    }

    #[test]
    fn a_pixel_radius_applies_to_every_corner() {
        assert_eq!(radii("width:100px;height:50px;border-radius:12px"), [[12.0, 12.0]; 4]);
    }

    #[test]
    fn a_percentage_is_elliptical_on_a_rectangle() {
        // 50% of a 200x80 box is 100 across and 40 down, not a circle.
        assert_eq!(radii("width:200px;height:80px;border-radius:50%"), [[100.0, 40.0]; 4]);
    }

    #[test]
    fn corners_can_differ() {
        let r = radii("width:200px;height:100px;border-radius:10px 20px 30px 40px");
        assert_eq!(r, [[10.0, 10.0], [20.0, 20.0], [30.0, 30.0], [40.0, 40.0]]);
    }

    #[test]
    fn overlapping_radii_shrink_by_one_shared_factor() {
        // 500px of radius on a 100x60 box. CSS picks a single f for every
        // corner and both axes, so the tightest side wins: 60/(500+500) =
        // 0.06, giving 30x30 corners. That is a stadium with semicircular
        // ends, not an ellipse half the box — and it is what Chromium draws.
        let r = radii("width:100px;height:60px;border-radius:500px");
        assert!(r.iter().all(|c| *c == [30.0, 30.0]), "corners diverged: {r:?}");
    }

    #[test]
    fn a_radius_that_already_fits_is_left_alone() {
        let r = radii("width:200px;height:100px;border-radius:20px");
        assert!(r.iter().all(|c| *c == [20.0, 20.0]), "shrunk without need: {r:?}");
    }
}
