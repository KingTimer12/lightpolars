//! State of one open page.

/// Default viewport Puppeteer assumes when nothing is emulated.
pub const DEFAULT_VIEWPORT: (f32, f32) = (800.0, 600.0);

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
        }
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
}

impl Default for PageState {
    fn default() -> Self {
        Self::new(String::new(), String::new())
    }
}
