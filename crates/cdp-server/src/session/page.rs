//! State of one open page.

use render_ir::DisplayList;
use std::sync::Arc;

/// Default viewport Puppeteer assumes when nothing is emulated.
pub const DEFAULT_VIEWPORT: (f32, f32) = (800.0, 600.0);

/// How many laid-out widths to keep per page.
///
/// Two, because a page is normally asked for both outputs and they lay out at
/// different widths: the PDF uses the sheet minus its margins, the screenshot
/// uses the viewport. One slot would make those two thrash each other.
const CACHED_WIDTHS: usize = 2;

/// The consumer API opens one page per request, so each needs its own sessionId
/// and frameId — reusing them makes the second page collide with the first on
/// the Puppeteer side.
#[derive(Debug, Clone)]
pub struct PageState {
    pub target_id: String,
    pub frame_id: String,
    pub html: Option<String>,
    /// Screenshot viewport, in CSS px. The PDF does not use it: there the width
    /// comes from the sheet and the margins of `Page.printToPDF`.
    pub viewport_width: f32,
    pub viewport_height: f32,
    /// `deviceScaleFactor` from Emulation.setDeviceMetricsOverride.
    pub device_scale_factor: f32,
    /// Display lists already computed for this HTML, keyed by layout width.
    ///
    /// Laying out is the expensive half of every command, and a single
    /// screenshot asks for it twice: Puppeteer calls `Page.getLayoutMetrics`
    /// to size the clip and then `Page.captureScreenshot` to draw it, both on
    /// the same HTML at the same width. The list is behind an `Arc` because
    /// `Session::page` hands out clones of this struct.
    ///
    /// Emptied whenever the HTML changes — that is the only input to it, since
    /// the width is part of the key and the device scale factor is applied
    /// after layout.
    layouts: Vec<(f32, Arc<DisplayList>)>,
}

impl PageState {
    pub fn new(target_id: String, frame_id: String) -> Self {
        Self {
            target_id,
            frame_id,
            html: None,
            viewport_width: DEFAULT_VIEWPORT.0,
            viewport_height: DEFAULT_VIEWPORT.1,
            device_scale_factor: 1.0,
            layouts: Vec::new(),
        }
    }

    /// Replaces the document and drops everything laid out from the old one.
    pub fn set_html(&mut self, html: Option<String>) {
        self.html = html;
        self.layouts.clear();
    }

    /// The display list for this page at `width_px`, laying it out only if no
    /// previous command already did at the same width.
    pub fn layout(&mut self, width_px: f32) -> Arc<DisplayList> {
        // Bitwise comparison rather than an epsilon: the widths come from the
        // same arithmetic on the same params, so a hit is an exact hit, and a
        // near-miss must lay out again anyway to be correct.
        if let Some((_, list)) = self.layouts.iter().find(|(w, _)| w.to_bits() == width_px.to_bits())
        {
            return list.clone();
        }

        let list = Arc::new(render_core::render_html(
            self.html.as_deref().unwrap_or_default(),
            width_px,
        ));
        if self.layouts.len() == CACHED_WIDTHS {
            self.layouts.remove(0);
        }
        self.layouts.push((width_px, list.clone()));
        list
    }

    /// Reverts what `Emulation.setDeviceMetricsOverride` changed.
    pub fn clear_device_metrics(&mut self) {
        self.viewport_width = DEFAULT_VIEWPORT.0;
        self.viewport_height = DEFAULT_VIEWPORT.1;
        self.device_scale_factor = 1.0;
    }

    pub fn html(&self) -> String {
        self.html.clone().unwrap_or_default()
    }

    /// Number of widths currently laid out. For tests.
    #[cfg(test)]
    pub fn cached_layouts(&self) -> usize {
        self.layouts.len()
    }
}

impl Default for PageState {
    fn default() -> Self {
        Self::new(String::new(), String::new())
    }
}
