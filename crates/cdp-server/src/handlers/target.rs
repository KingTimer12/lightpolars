//! Browser and target lifecycle: creating, attaching to and closing pages.

use crate::session::{Command, Output, PageState, Session, target_info};
use serde_json::json;

pub fn handle(session: &mut Session, cmd: &Command) -> Vec<Output> {
    match cmd.method.as_str() {
        "Browser.getVersion" => vec![cmd.ok(json!({
            "protocolVersion": "1.3",
            "product": "HeadlessChrome/0.0.0",
            "revision": "0",
            "userAgent": "cdp-server",
            "jsVersion": "0"
        }))],
        "Target.getBrowserContexts" => vec![cmd.ok(json!({ "browserContextIds": [] }))],
        "Target.createTarget" => create_target(session, cmd),
        "Target.attachToTarget" => attach_to_target(session, cmd),
        "Target.getTargetInfo" => {
            let tid = session
                .last_target
                .clone()
                .unwrap_or_else(|| "target-1".to_string());
            vec![cmd.ok(json!({ "targetInfo": target_info(&tid) }))]
        }
        _ => close_target(session, cmd),
    }
}

fn create_target(session: &mut Session, cmd: &Command) -> Vec<Output> {
    session.page_counter += 1;
    let n = session.page_counter;
    let target_id = format!("target-{n}");
    let session_id = format!("session-{n}");
    session.last_target = Some(target_id.clone());
    session.pages.insert(
        session_id.clone(),
        PageState::new(target_id.clone(), format!("frame-{n}")),
    );

    // Puppeteer never calls attachToTarget: it turns on Target.setAutoAttach
    // and expects attachedToTarget to arrive with the target. Without it,
    // Browser.waitForTarget hangs until the protocolTimeout.
    vec![
        cmd.event("Target.targetCreated", json!({ "targetInfo": target_info(&target_id) })),
        cmd.ok(json!({ "targetId": &target_id })),
        cmd.event(
            "Target.attachedToTarget",
            json!({
                "sessionId": session_id,
                "targetInfo": target_info(&target_id),
                "waitingForDebugger": false
            }),
        ),
    ]
}

fn attach_to_target(session: &mut Session, cmd: &Command) -> Vec<Output> {
    let target_id = cmd
        .string("targetId")
        .map(str::to_string)
        .or_else(|| session.last_target.clone())
        .unwrap_or_else(|| "target-1".to_string());
    let session_id = session
        .session_of_target(&target_id)
        .unwrap_or_else(|| "session-1".to_string());

    vec![
        // The event comes before the response: Puppeteer registers the
        // CDPSession when it sees it, and only then matches the result.
        cmd.event(
            "Target.attachedToTarget",
            json!({
                "sessionId": session_id,
                "targetInfo": target_info(&target_id),
                "waitingForDebugger": false
            }),
        ),
        cmd.ok(json!({ "sessionId": session_id })),
    ]
}

fn close_target(session: &mut Session, cmd: &Command) -> Vec<Output> {
    let target_id = cmd
        .string("targetId")
        .map(str::to_string)
        .unwrap_or_else(|| session.page(&cmd.session).target_id);
    let session_id = session.session_of_target(&target_id);
    if let Some(s) = &session_id {
        session.pages.remove(s);
    }

    // page.close() awaits _isClosedDeferred, which only resolves on
    // Target.detachedFromTarget. Without these two events close() never
    // returns and the next page is never opened.
    let mut out = vec![cmd.ok(json!({ "success": true }))];
    if let Some(s) = session_id {
        out.push(cmd.browser_event(
            "Target.detachedFromTarget",
            json!({ "sessionId": s, "targetId": &target_id }),
        ));
    }
    out.push(cmd.browser_event("Target.targetDestroyed", json!({ "targetId": target_id })));
    out
}
