//! Protocol-level tests against the whole session state machine.

use base64::Engine as _;
use cdp_server::session::{Output, Session};
use serde_json::{Value, json};

fn parse(s: &str) -> Value {
    serde_json::from_str(s).expect("output is not valid JSON")
}

fn responses(outputs: &[Output]) -> Vec<Value> {
    outputs
        .iter()
        .filter_map(|o| match o {
            Output::Response(t) => Some(parse(t)),
            _ => None,
        })
        .collect()
}

fn events(outputs: &[Output]) -> Vec<Value> {
    outputs
        .iter()
        .filter_map(|o| match o {
            Output::Event(t) => Some(parse(t)),
            _ => None,
        })
        .collect()
}

fn send(session: &mut Session, id: i64, method: &str, params: Value) -> Vec<Output> {
    let msg = json!({ "id": id, "method": method, "params": params });
    session.handle(&msg.to_string())
}

/// A session whose implicit page already holds the given HTML.
fn with_html(html: &str) -> Session {
    let mut s = Session::new();
    send(&mut s, 1, "Page.setDocumentContent", json!({ "frameId": "f", "html": html }));
    s
}

// --- target lifecycle ---

#[test]
fn create_target_answers_with_a_target_id() {
    let mut s = Session::new();
    let out = send(&mut s, 1, "Target.createTarget", json!({ "url": "about:blank" }));
    let r = responses(&out);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0]["id"], 1);
    assert!(r[0]["result"]["targetId"].is_string());
}

#[test]
fn create_target_emits_attached_to_target_unprompted() {
    // Puppeteer 25 never calls attachToTarget; it relies on this event.
    let mut s = Session::new();
    let out = send(&mut s, 1, "Target.createTarget", json!({ "url": "about:blank" }));
    let names: Vec<String> = events(&out)
        .iter()
        .map(|e| e["method"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(names.contains(&"Target.targetCreated".to_string()), "{names:?}");
    assert!(names.contains(&"Target.attachedToTarget".to_string()), "{names:?}");
}

#[test]
fn closing_a_target_emits_detached_then_destroyed() {
    let mut s = Session::new();
    send(&mut s, 1, "Target.createTarget", json!({ "url": "about:blank" }));
    let out = send(&mut s, 2, "Target.closeTarget", json!({ "targetId": "target-1" }));
    let names: Vec<String> = events(&out)
        .iter()
        .map(|e| e["method"].as_str().unwrap_or_default().to_string())
        .collect();
    // page.close() awaits detachedFromTarget; without it close() never returns.
    assert_eq!(
        names,
        vec!["Target.detachedFromTarget", "Target.targetDestroyed"],
        "{names:?}"
    );
}

#[test]
fn attach_to_target_emits_the_event_before_the_response() {
    // Puppeteer registers the CDPSession on the event; a response arriving
    // first has no session to match against.
    let mut s = Session::new();
    send(&mut s, 1, "Target.createTarget", json!({ "url": "about:blank" }));
    let out = send(&mut s, 2, "Target.attachToTarget", json!({ "targetId": "target-1" }));
    assert!(matches!(out.first(), Some(Output::Event(_))), "event must come first");
    assert!(matches!(out.last(), Some(Output::Response(_))));
    let sid = responses(&out)[0]["result"]["sessionId"].as_str().unwrap().to_string();
    assert_eq!(sid, "session-1", "attach must find the existing page session");
}

#[test]
fn pages_opened_in_sequence_get_their_own_session_and_target() {
    let mut s = Session::new();
    let mut seen = Vec::new();
    for id in 1..=2 {
        let out = send(&mut s, id, "Target.createTarget", json!({ "url": "about:blank" }));
        let attached = events(&out)
            .into_iter()
            .find(|e| e["method"] == "Target.attachedToTarget")
            .expect("no attachedToTarget");
        seen.push((
            attached["params"]["sessionId"].as_str().unwrap().to_string(),
            attached["params"]["targetInfo"]["targetId"].as_str().unwrap().to_string(),
        ));
    }
    assert_ne!(seen[0].0, seen[1].0, "session ids collided");
    assert_ne!(seen[0].1, seen[1].1, "target ids collided");
}

#[test]
fn a_command_without_a_session_id_does_not_gain_the_field() {
    let mut s = Session::new();
    let out = send(&mut s, 1, "Page.getFrameTree", json!({}));
    for value in responses(&out).iter().chain(events(&out).iter()) {
        assert!(value.get("sessionId").is_none(), "unexpected sessionId: {value}");
    }
}

#[test]
fn get_frame_tree_returns_a_real_frame_object() {
    // Returning `{}` makes the client crash reading `frameTree.frame`.
    let mut s = Session::new();
    let r = responses(&send(&mut s, 1, "Page.getFrameTree", json!({})));
    let frame = &r[0]["result"]["frameTree"]["frame"];
    assert!(frame["id"].is_string(), "{frame}");
    assert_eq!(frame["url"], "about:blank");
    assert_eq!(frame["mimeType"], "text/html");
    assert!(r[0]["result"]["frameTree"]["childFrames"].is_array());
}

#[test]
fn the_command_session_id_comes_back_on_the_response_and_events() {
    let mut s = Session::new();
    let msg = json!({
        "id": 7,
        "sessionId": "session-abc",
        "method": "Page.setDocumentContent",
        "params": { "frameId": "f", "html": "<p>x</p>" }
    });
    let out = s.handle(&msg.to_string());
    for value in responses(&out).iter().chain(events(&out).iter()) {
        assert_eq!(value["sessionId"], "session-abc", "{value}");
    }
}

#[test]
fn an_unknown_method_gets_an_empty_result_instead_of_silence() {
    let mut s = Session::new();
    let r = responses(&send(&mut s, 3, "Some.unknownMethod", json!({})));
    assert_eq!(r.len(), 1);
    assert_eq!(r[0]["id"], 3);
    assert!(r[0]["result"].is_object());
}

#[test]
fn malformed_json_is_dropped_without_panicking() {
    let mut s = Session::new();
    assert!(s.handle("{ not json").is_empty());
}

// --- page content ---

#[test]
fn set_document_content_emits_the_full_lifecycle_before_the_response() {
    let mut s = Session::new();
    let out = send(
        &mut s,
        1,
        "Page.setDocumentContent",
        json!({ "frameId": "f", "html": "<p>x</p>" }),
    );
    let names: Vec<String> = events(&out)
        .iter()
        .map(|e| e["params"]["name"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(names, vec!["init", "load", "DOMContentLoaded", "networkIdle"]);
    // The response comes last, so the LifecycleWatcher never races it.
    assert!(matches!(out.last(), Some(Output::Response(_))));
}

#[test]
fn each_page_keeps_its_own_html_frame_and_session() {
    let mut s = Session::new();
    send(&mut s, 1, "Target.createTarget", json!({ "url": "about:blank" }));
    send(&mut s, 2, "Target.createTarget", json!({ "url": "about:blank" }));

    for (n, html) in [(1, "<p>um</p>"), (2, "<p>dois</p>")] {
        let msg = json!({
            "id": 10 + n,
            "sessionId": format!("session-{n}"),
            "method": "Page.setDocumentContent",
            "params": { "frameId": "f", "html": html }
        });
        s.handle(&msg.to_string());
    }

    assert_eq!(s.html_for_session("session-1"), Some("<p>um</p>"));
    assert_eq!(s.html_for_session("session-2"), Some("<p>dois</p>"));

    let frame_of = |s: &mut Session, n: i64| -> String {
        let msg = json!({
            "id": 20 + n,
            "sessionId": format!("session-{n}"),
            "method": "Page.getFrameTree",
            "params": {}
        });
        responses(&s.handle(&msg.to_string()))[0]["result"]["frameTree"]["frame"]["id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_ne!(frame_of(&mut s, 1), frame_of(&mut s, 2), "frame ids collided");
}

// --- printToPDF ---

#[test]
fn print_to_pdf_returns_a_base64_pdf() {
    let mut s = with_html("<p>oi</p>");
    let r = responses(&send(
        &mut s,
        2,
        "Page.printToPDF",
        json!({
            "paperWidth": 8.27, "paperHeight": 11.7,
            "marginTop": 0, "marginRight": 0, "marginBottom": 0, "marginLeft": 0,
            "printBackground": true
        }),
    ));
    let data = r[0]["result"]["data"].as_str().expect("no data field");
    let bytes = base64::engine::general_purpose::STANDARD.decode(data).unwrap();
    assert!(bytes.starts_with(b"%PDF-"));
}

#[test]
fn print_to_pdf_without_prior_content_does_not_panic() {
    let mut s = Session::new();
    let r = responses(&send(&mut s, 1, "Page.printToPDF", json!({})));
    assert!(r[0]["result"]["data"].is_string());
}

#[test]
fn reading_an_unknown_stream_handle_ends_instead_of_panicking() {
    let mut s = Session::new();
    let r = responses(&send(&mut s, 1, "IO.read", json!({ "handle": "nope" })));
    assert_eq!(r[0]["result"]["eof"], true);
    assert_eq!(r[0]["result"]["data"], "");
}

#[test]
fn a_stream_transfer_is_drained_by_io_read() {
    let mut s = with_html("<p>oi</p>");
    let r = responses(&send(
        &mut s,
        2,
        "Page.printToPDF",
        json!({ "transferMode": "ReturnAsStream", "paperWidth": 8.27, "paperHeight": 11.7 }),
    ));
    let handle = r[0]["result"]["stream"].as_str().expect("no stream handle");

    let mut bytes = Vec::new();
    loop {
        let chunk = responses(&send(&mut s, 3, "IO.read", json!({ "handle": handle })));
        let data = chunk[0]["result"]["data"].as_str().unwrap();
        bytes.extend(base64::engine::general_purpose::STANDARD.decode(data).unwrap());
        if chunk[0]["result"]["eof"].as_bool().unwrap() {
            break;
        }
    }
    assert!(bytes.starts_with(b"%PDF-"));

    send(&mut s, 4, "IO.close", json!({ "handle": handle }));
    // A closed handle reads as an immediate EOF rather than an error.
    let after = responses(&send(&mut s, 5, "IO.read", json!({ "handle": handle })));
    assert_eq!(after[0]["result"]["eof"], true);
}

// --- captureScreenshot ---

fn screenshot(s: &mut Session, params: Value) -> Vec<u8> {
    let r = responses(&send(s, 9, "Page.captureScreenshot", params));
    assert_eq!(r.len(), 1);
    let data = r[0]["result"]["data"]
        .as_str()
        .unwrap_or_else(|| panic!("no data in response: {}", r[0]));
    base64::engine::general_purpose::STANDARD.decode(data).unwrap()
}

/// Width and height read from the PNG IHDR header (bytes 16..24).
fn png_size(bytes: &[u8]) -> (u32, u32) {
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "not a PNG");
    let n = |i: usize| u32::from_be_bytes(bytes[i..i + 4].try_into().unwrap());
    (n(16), n(20))
}

#[test]
fn a_screenshot_returns_a_png_the_size_of_the_viewport() {
    let mut s = with_html("<p>oi</p>");
    let bytes = screenshot(&mut s, json!({}));
    assert_eq!(png_size(&bytes), (800, 600), "Puppeteer's default viewport");
}

#[test]
fn capture_beyond_viewport_uses_the_content_height() {
    let mut s = with_html(r#"<div style="height:2000px"></div>"#);
    let bytes = screenshot(&mut s, json!({ "captureBeyondViewport": true }));
    let (_, height) = png_size(&bytes);
    assert!(height >= 2000, "content of 2000px came out {height}px tall");
}

#[test]
fn a_clip_sets_the_image_size_and_scale() {
    let mut s = with_html("<p>oi</p>");
    let bytes = screenshot(
        &mut s,
        json!({ "clip": { "x": 0, "y": 0, "width": 100, "height": 50, "scale": 2 } }),
    );
    assert_eq!(png_size(&bytes), (200, 100));
}

#[test]
fn the_device_scale_factor_multiplies_the_screenshot() {
    let mut s = with_html("<p>oi</p>");
    send(
        &mut s,
        2,
        "Emulation.setDeviceMetricsOverride",
        json!({ "width": 400, "height": 300, "deviceScaleFactor": 2 }),
    );
    assert_eq!(png_size(&screenshot(&mut s, json!({}))), (800, 600), "400x300 at 2x");
}

#[test]
fn clearing_device_metrics_restores_the_default_viewport() {
    let mut s = with_html("<p>oi</p>");
    send(
        &mut s,
        2,
        "Emulation.setDeviceMetricsOverride",
        json!({ "width": 400, "height": 300, "deviceScaleFactor": 2 }),
    );
    send(&mut s, 3, "Emulation.clearDeviceMetricsOverride", json!({}));
    assert_eq!(png_size(&screenshot(&mut s, json!({}))), (800, 600));
}

#[test]
fn an_emulated_viewport_changes_the_layout_width() {
    let mut s = with_html("<p>oi</p>");
    send(
        &mut s,
        2,
        "Emulation.setDeviceMetricsOverride",
        json!({ "width": 1200, "height": 400, "deviceScaleFactor": 1 }),
    );
    assert_eq!(png_size(&screenshot(&mut s, json!({}))), (1200, 400));
}

#[test]
fn a_jpeg_screenshot_carries_the_jpeg_marker() {
    let mut s = with_html("<p>oi</p>");
    let bytes = screenshot(&mut s, json!({ "format": "jpeg", "quality": 60 }));
    assert_eq!(&bytes[..2], b"\xff\xd8", "JPEG SOI");
}

#[test]
fn an_unsupported_format_errors_instead_of_returning_a_disguised_png() {
    let mut s = with_html("<p>oi</p>");
    let r = responses(&send(&mut s, 9, "Page.captureScreenshot", json!({ "format": "webp" })));
    assert!(r[0]["result"].is_null(), "should not answer with success");
    assert_eq!(r[0]["error"]["code"], -32000);
    assert!(
        r[0]["error"]["message"].as_str().unwrap().contains("webp"),
        "the message should name the format: {}",
        r[0]["error"]["message"]
    );
}

#[test]
fn a_screenshot_without_html_does_not_panic() {
    let mut s = Session::new();
    assert_eq!(png_size(&screenshot(&mut s, json!({}))), (800, 600));
}

#[test]
fn layout_metrics_report_the_real_content_height() {
    let mut s = with_html(r#"<div style="height:3000px"></div>"#);
    let r = responses(&send(&mut s, 5, "Page.getLayoutMetrics", json!({})));
    let height = r[0]["result"]["cssContentSize"]["height"].as_f64().unwrap();
    assert!(height >= 3000.0, "contentSize came out {height}");
    assert_eq!(r[0]["result"]["cssLayoutViewport"]["clientWidth"], 800.0);
}

// --- runtime stubs ---

#[test]
fn runtime_enable_creates_a_main_world_context() {
    let mut s = Session::new();
    let out = send(&mut s, 1, "Runtime.enable", json!({}));
    let e = events(&out);
    assert_eq!(e[0]["method"], "Runtime.executionContextCreated");
    assert_eq!(e[0]["params"]["context"]["auxData"]["isDefault"], true);
}

#[test]
fn an_isolated_world_gets_its_own_context_id() {
    let mut s = Session::new();
    send(&mut s, 1, "Runtime.enable", json!({}));
    let out = send(&mut s, 2, "Page.createIsolatedWorld", json!({ "worldName": "util" }));
    let e = events(&out);
    assert_eq!(e[0]["params"]["context"]["auxData"]["isDefault"], false);
    assert_eq!(e[0]["params"]["context"]["name"], "util");
    let r = responses(&out);
    assert_eq!(r[0]["result"]["executionContextId"], 2);
}

#[test]
fn evaluate_always_answers_undefined() {
    // page.pdf()'s waitForFonts calls document.fonts.ready; with no stub it
    // would hang forever.
    let mut s = Session::new();
    for method in ["Runtime.evaluate", "Runtime.callFunctionOn"] {
        let r = responses(&send(&mut s, 1, method, json!({})));
        assert_eq!(r[0]["result"]["result"]["type"], "undefined", "{method}");
    }
}
