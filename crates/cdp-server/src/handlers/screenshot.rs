//! `Page.captureScreenshot`, the layout metrics it needs and the viewport
//! emulation that sizes it.

use crate::session::{Command, Output, PageState, Session};
use paginate::{Page, PageGeometry};
use render_ir::{DisplayList, Rect};
use serde_json::{Value, json};

pub fn handle(session: &mut Session, cmd: &Command) -> Vec<Output> {
    match cmd.method.as_str() {
        "Page.captureScreenshot" => match capture(session, cmd) {
            Ok(bytes) => {
                use base64::Engine as _;
                let data = base64::engine::general_purpose::STANDARD.encode(bytes);
                vec![cmd.ok(json!({ "data": data }))]
            }
            // Returning a PNG where the client asked for WebP would be worse
            // than failing: it would save the file with the wrong extension.
            Err(reason) => vec![cmd.error(&reason)],
        },
        "Page.getLayoutMetrics" => layout_metrics(session, cmd),
        "Emulation.setDeviceMetricsOverride" => set_device_metrics(session, cmd),
        _ => {
            session.page_mut(&cmd.session).clear_device_metrics();
            vec![cmd.ok(json!({}))]
        }
    }
}

/// Puppeteer uses this to build the clip of a full-page screenshot; returning
/// zeros would make the image come out empty.
fn layout_metrics(session: &mut Session, cmd: &Command) -> Vec<Output> {
    let page = session.page(&cmd.session);
    let dl = session.page_mut(&cmd.session).layout(page.viewport_width);
    let height = dl.content_height().max(page.viewport_height);

    let content = json!({ "x": 0, "y": 0, "width": page.viewport_width, "height": height });
    let viewport = json!({
        "x": 0, "y": 0,
        "width": page.viewport_width,
        "height": page.viewport_height,
        "clientWidth": page.viewport_width,
        "clientHeight": page.viewport_height,
        "pageX": 0, "pageY": 0, "scale": 1
    });

    vec![cmd.ok(json!({
        "layoutViewport": viewport,
        "visualViewport": viewport,
        "contentSize": content,
        "cssLayoutViewport": viewport,
        "cssVisualViewport": viewport,
        "cssContentSize": content
    }))]
}

fn set_device_metrics(session: &mut Session, cmd: &Command) -> Vec<Output> {
    let number = |key: &str| cmd.params.get(key).and_then(Value::as_f64);
    let page = session.page_mut(&cmd.session);
    // Width/height of 0 mean "use the default", not a null window.
    if let Some(w) = number("width").filter(|w| *w > 0.0) {
        page.viewport_width = w as f32;
    }
    if let Some(h) = number("height").filter(|h| *h > 0.0) {
        page.viewport_height = h as f32;
    }
    if let Some(s) = number("deviceScaleFactor").filter(|s| *s > 0.0) {
        page.device_scale_factor = s as f32;
    }
    vec![cmd.ok(json!({}))]
}

/// Draws the page as an image. Returns `Err` with the CDP error message when
/// the requested format is not supported.
fn capture(session: &mut Session, cmd: &Command) -> Result<Vec<u8>, String> {
    let format = cmd.string("format").unwrap_or("png").to_ascii_lowercase();
    if format != "png" && format != "jpeg" && format != "jpg" {
        return Err(format!(
            "captureScreenshot: format '{format}' is not supported (use png or jpeg)"
        ));
    }

    let state = session.page(&cmd.session);
    let dl = session.page_mut(&cmd.session).layout(state.viewport_width);
    let (page, geo, scale) = frame_to_draw(cmd, &state, &dl);

    let bytes = if format == "png" {
        raster_out::render_png(&page, &dl.fonts, &geo, scale)
    } else {
        let quality = cmd.params.get("quality").and_then(Value::as_u64).unwrap_or(80);
        raster_out::render_jpeg(&page, &dl.fonts, &geo, scale, quality.min(100) as u8)
    };
    bytes.ok_or_else(|| "captureScreenshot: invalid geometry for the image".to_string())
}

/// Which part of the document to draw, how large, and at what scale.
fn frame_to_draw(
    cmd: &Command,
    state: &PageState,
    dl: &DisplayList,
) -> (Page, PageGeometry, f32) {
    if let Some(clip) = cmd.params.get("clip").filter(|c| c.is_object()) {
        let number = |key: &str, default: f64| clip.get(key).and_then(Value::as_f64).unwrap_or(default);
        let area = Rect {
            x: number("x", 0.0) as f32,
            y: number("y", 0.0) as f32,
            width: number("width", state.viewport_width as f64) as f32,
            height: number("height", state.viewport_height as f64) as f32,
        };
        let (page, geo) = paginate::clip(dl, area);
        // With a clip Chromium uses `clip.scale` as the final factor; the
        // deviceScaleFactor is already baked into the measurements the client
        // computed from getLayoutMetrics.
        return (page, geo, number("scale", 1.0) as f32);
    }

    let beyond_viewport = cmd
        .params
        .get("captureBeyondViewport")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let (page, mut geo) = paginate::whole_document(dl);
    geo.sheet_width = state.viewport_width;
    // Without captureBeyondViewport the image is the size of the window, even
    // if the content is shorter (background shows) or taller (it is cropped).
    geo.sheet_height = if beyond_viewport {
        geo.sheet_height.max(state.viewport_height)
    } else {
        state.viewport_height
    };
    (page, geo, state.device_scale_factor)
}
