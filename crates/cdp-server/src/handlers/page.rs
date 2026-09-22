//! Page content and navigation state.

use crate::session::{Command, Output, Session, frame_info};
use serde_json::json;

pub fn handle(session: &mut Session, cmd: &Command) -> Vec<Output> {
    match cmd.method.as_str() {
        "Page.setDocumentContent" => set_document_content(session, cmd),
        "Page.getFrameTree" => {
            let frame = frame_info(&session.page(&cmd.session).frame_id);
            vec![cmd.ok(json!({ "frameTree": { "frame": frame, "childFrames": [] } }))]
        }
        _ => vec![cmd.ok(json!({
            "currentIndex": 0,
            "entries": [{
                "id": 1,
                "url": "about:blank",
                "userTypedURL": "about:blank",
                "title": "",
                "transitionType": "typed"
            }]
        }))],
    }
}

fn set_document_content(session: &mut Session, cmd: &Command) -> Vec<Output> {
    session
        .page_mut(&cmd.session)
        .set_html(cmd.string("html").map(str::to_string));
    let frame_id = session.page(&cmd.session).frame_id;

    // The events go out before the response: Puppeteer's LifecycleWatcher only
    // starts resolving after it sees the full cycle, and this way the order on
    // the wire does not depend on a race.
    let mut out: Vec<Output> = ["init", "load", "DOMContentLoaded", "networkIdle"]
        .into_iter()
        .map(|name| {
            cmd.event(
                "Page.lifecycleEvent",
                json!({
                    "frameId": frame_id,
                    "loaderId": "loader-1",
                    "name": name,
                    "timestamp": 0.0
                }),
            )
        })
        .collect();
    out.push(cmd.ok(json!({})));
    out
}
