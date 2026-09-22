//! Execution contexts and the JavaScript stubs.
//!
//! There is no JavaScript engine: every evaluation returns undefined. The only
//! use on the PDF path is `document.fonts.ready`, and fonts are already
//! resolved synchronously during layout.

use crate::session::{Command, Output, Session, execution_context};
use serde_json::json;

pub fn handle(session: &mut Session, cmd: &Command) -> Vec<Output> {
    match cmd.method.as_str() {
        // Main-world context: without it the FrameManager never binds the
        // frame to a realm and any evaluate hangs.
        "Runtime.enable" => {
            let (ctx, frame_id) = new_context(session, cmd);
            vec![
                cmd.event(
                    "Runtime.executionContextCreated",
                    json!({ "context": execution_context(&frame_id, ctx, "", true) }),
                ),
                cmd.ok(json!({})),
            ]
        }
        "Page.createIsolatedWorld" => {
            let (ctx, frame_id) = new_context(session, cmd);
            let name = cmd.string("worldName").unwrap_or("").to_string();
            vec![
                cmd.event(
                    "Runtime.executionContextCreated",
                    json!({ "context": execution_context(&frame_id, ctx, &name, false) }),
                ),
                cmd.ok(json!({ "executionContextId": ctx })),
            ]
        }
        "Page.addScriptToEvaluateOnNewDocument" => {
            vec![cmd.ok(json!({ "identifier": "1" }))]
        }
        _ => vec![cmd.ok(json!({ "result": { "type": "undefined" } }))],
    }
}

fn new_context(session: &mut Session, cmd: &Command) -> (i64, String) {
    let ctx = session.next_context_id();
    (ctx, session.page(&cmd.session).frame_id)
}
