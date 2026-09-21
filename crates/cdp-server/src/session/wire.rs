//! The CDP message shapes: what goes on the wire in each direction.
//!
//! Isolated from the handlers so that "what a response looks like" is decided
//! in one place, and so the flatten-mode `sessionId` is never forgotten on an
//! individual reply.

use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq)]
pub enum Output {
    Response(String),
    Event(String),
}

/// One inbound command, already parsed.
pub struct Command {
    pub id: i64,
    pub method: String,
    pub params: Value,
    /// Flatten mode: every message from an attached session carries a
    /// sessionId, and the reply must echo it or Puppeteer never matches it to
    /// the command and waits forever.
    pub session: Option<String>,
}

impl Command {
    pub fn parse(msg: &str) -> Option<Self> {
        let v: Value = serde_json::from_str(msg).ok()?;
        Some(Self {
            id: v.get("id").and_then(Value::as_i64).unwrap_or(0),
            method: v
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            params: v.get("params").cloned().unwrap_or_else(|| json!({})),
            session: v.get("sessionId").and_then(Value::as_str).map(str::to_string),
        })
    }

    /// A numeric param, or `default` when absent or not a number.
    pub fn number(&self, key: &str, default: f64) -> f64 {
        self.params.get(key).and_then(Value::as_f64).unwrap_or(default)
    }

    pub fn string(&self, key: &str) -> Option<&str> {
        self.params.get(key).and_then(Value::as_str)
    }

    pub fn ok(&self, result: Value) -> Output {
        response(self.id, &self.session, result)
    }

    /// A CDP error reply. `-32000` is the server-error code Puppeteer turns
    /// into an exception at the command's `await`.
    pub fn error(&self, message: &str) -> Output {
        let mut msg = json!({ "id": self.id, "error": { "code": -32000, "message": message } });
        attach_session_id(&mut msg, &self.session);
        Output::Response(msg.to_string())
    }

    pub fn event(&self, method: &str, params: Value) -> Output {
        event_for(&self.session, method, params)
    }

    /// An event that belongs to the browser, not to a page session.
    pub fn browser_event(&self, method: &str, params: Value) -> Output {
        event_for(&None, method, params)
    }
}

fn response(id: i64, session: &Option<String>, result: Value) -> Output {
    let mut msg = json!({ "id": id, "result": result });
    attach_session_id(&mut msg, session);
    Output::Response(msg.to_string())
}

fn event_for(session: &Option<String>, method: &str, params: Value) -> Output {
    let mut msg = json!({ "method": method, "params": params });
    attach_session_id(&mut msg, session);
    Output::Event(msg.to_string())
}

fn attach_session_id(msg: &mut Value, session: &Option<String>) {
    if let (Some(obj), Some(sid)) = (msg.as_object_mut(), session.as_ref()) {
        obj.insert("sessionId".into(), Value::String(sid.clone()));
    }
}

/// The `Target.targetInfo` payload, identical in every message carrying it.
pub fn target_info(target_id: &str) -> Value {
    json!({
        "targetId": target_id,
        "type": "page",
        "title": "",
        "url": "about:blank",
        "attached": true,
        "canAccessOpener": false,
        "browserContextId": "context-1"
    })
}

pub fn frame_info(frame_id: &str) -> Value {
    json!({
        "id": frame_id,
        "loaderId": "loader-1",
        "url": "about:blank",
        "domainAndRegistry": "",
        "securityOrigin": "://",
        "mimeType": "text/html",
        "secureContextType": "Secure",
        "crossOriginIsolatedContextType": "NotIsolated",
        "gatedAPIFeatures": []
    })
}

pub fn execution_context(frame_id: &str, id: i64, name: &str, main_world: bool) -> Value {
    json!({
        "id": id,
        "origin": "://",
        "name": name,
        "uniqueId": format!("ctx-{id}"),
        "auxData": {
            "frameId": frame_id,
            "isDefault": main_world,
            "type": if main_world { "default" } else { "isolated" }
        }
    })
}
