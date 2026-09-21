//! CDP state machine, independent of transport.
//!
//! This module owns the state and the routing table only; each group of
//! methods is implemented in its own handler under `crate::handlers`. That
//! split exists because the dispatch used to be one 30-arm match doing target
//! lifecycle, page lifecycle, PDF, screenshots, streams and runtime stubs at
//! once.

mod page;
mod streams;
mod wire;

pub use page::{DEFAULT_VIEWPORT, PageState};
pub use streams::StreamStore;
pub use wire::{Command, Output};
pub(crate) use wire::{execution_context, frame_info, target_info};

use crate::handlers;
use std::collections::HashMap;

/// Session key used by callers that speak raw CDP without attaching a session.
const IMPLICIT_SESSION: &str = "session-implicita";

#[derive(Default)]
pub struct Session {
    pub(crate) page_counter: u32,
    pub(crate) context_counter: i64,
    pub(crate) last_target: Option<String>,
    /// Keyed by the sessionId Puppeteer puts on every command.
    pub(crate) pages: HashMap<String, PageState>,
    pub(crate) streams: StreamStore,
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn handle(&mut self, msg: &str) -> Vec<Output> {
        let Some(cmd) = Command::parse(msg) else {
            return Vec::new();
        };

        match cmd.method.as_str() {
            "Browser.getVersion" | "Target.getBrowserContexts" | "Target.createTarget"
            | "Target.attachToTarget" | "Target.getTargetInfo" | "Target.closeTarget"
            | "Target.detachFromTarget" | "Page.close" => handlers::target::handle(self, &cmd),

            "Page.setDocumentContent" | "Page.getFrameTree" | "Page.getNavigationHistory" => {
                handlers::page::handle(self, &cmd)
            }

            "Page.printToPDF" | "IO.read" | "IO.close" => handlers::print::handle(self, &cmd),

            "Page.captureScreenshot" | "Page.getLayoutMetrics"
            | "Emulation.setDeviceMetricsOverride" | "Emulation.clearDeviceMetricsOverride" => {
                handlers::screenshot::handle(self, &cmd)
            }

            "Runtime.enable" | "Page.createIsolatedWorld" | "Runtime.evaluate"
            | "Runtime.callFunctionOn" | "Page.addScriptToEvaluateOnNewDocument" => {
                handlers::runtime::handle(self, &cmd)
            }

            // Accepted with no effect: nothing goes to the network, no script
            // runs.
            _ => vec![cmd.ok(serde_json::json!({}))],
        }
    }

    /// The page a command targets: the one for the requested session, or the
    /// implicit one for callers speaking raw CDP with no session attached.
    pub(crate) fn page_mut(&mut self, session: &Option<String>) -> &mut PageState {
        let key = session.clone().unwrap_or_else(|| IMPLICIT_SESSION.to_string());
        self.pages
            .entry(key)
            .or_insert_with(|| PageState::new("target-implicito".into(), "frame-1".into()))
    }

    pub(crate) fn page(&self, session: &Option<String>) -> PageState {
        let key = session.clone().unwrap_or_else(|| IMPLICIT_SESSION.to_string());
        self.pages
            .get(&key)
            .cloned()
            .unwrap_or_else(|| PageState::new("target-implicito".into(), "frame-1".into()))
    }

    /// The sessionId that owns a target, if one is open.
    pub(crate) fn session_of_target(&self, target_id: &str) -> Option<String> {
        self.pages
            .iter()
            .find(|(_, p)| p.target_id == target_id)
            .map(|(s, _)| s.clone())
    }

    pub(crate) fn next_context_id(&mut self) -> i64 {
        self.context_counter += 1;
        self.context_counter
    }

    /// The HTML stored for a given sessionId, if that page exists.
    pub fn html_for_session(&self, session: &str) -> Option<&str> {
        self.pages.get(session)?.html.as_deref()
    }
}
