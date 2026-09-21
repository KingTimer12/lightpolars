//! Sheet dimensions and margins, in px at 96dpi.

pub const A4_WIDTH_PX: f32 = 793.7;
pub const A4_HEIGHT_PX: f32 = 1122.5;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Margins {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageGeometry {
    pub sheet_width: f32,
    pub sheet_height: f32,
    pub margins: Margins,
}

impl PageGeometry {
    pub fn a4(landscape: bool, margins: Margins) -> Self {
        let (w, h) = if landscape {
            (A4_HEIGHT_PX, A4_WIDTH_PX)
        } else {
            (A4_WIDTH_PX, A4_HEIGHT_PX)
        };
        Self { sheet_width: w, sheet_height: h, margins }
    }

    pub fn content_width(&self) -> f32 {
        self.sheet_width - self.margins.left - self.margins.right
    }

    pub fn content_height(&self) -> f32 {
        self.sheet_height - self.margins.top - self.margins.bottom
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zero_margins() -> Margins {
        Margins { top: 0.0, right: 0.0, bottom: 0.0, left: 0.0 }
    }

    #[test]
    fn a4_portrait_and_landscape_swap_sides() {
        let portrait = PageGeometry::a4(false, zero_margins());
        assert!((portrait.sheet_width - 793.7).abs() < 0.1);
        assert!((portrait.sheet_height - 1122.5).abs() < 0.1);

        let landscape = PageGeometry::a4(true, zero_margins());
        assert!((landscape.sheet_width - 1122.5).abs() < 0.1);
        assert!((landscape.sheet_height - 793.7).abs() < 0.1);
    }

    #[test]
    fn margins_shrink_the_content_area() {
        let m = Margins {
            top: render_ir::px_from_mm(51.0),
            right: render_ir::px_from_mm(15.0),
            bottom: render_ir::px_from_mm(20.0),
            left: render_ir::px_from_mm(15.0),
        };
        let geo = PageGeometry::a4(false, m);
        assert!((geo.content_width() - (793.7 - m.left - m.right)).abs() < 0.1);
        assert!((geo.content_height() - (1122.5 - m.top - m.bottom)).abs() < 0.1);
    }
}
