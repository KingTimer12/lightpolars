//! Resource provider for blitz, restricted to `data:`.
//!
//! SECURITY RULE: the HTML being processed is untrusted. Any scheme other than
//! `data:` is dropped without opening a socket or a file — the handler is never
//! even called, so blitz simply never receives that resource.

use blitz_dom::net::Resource;
use blitz_traits::net::{BoxedHandler, Bytes, NetCallback, NetProvider, Request};
use std::sync::{Arc, Mutex};

/// Collects resolved resources so the caller can apply them with
/// `Document::load_resource`. There is no event loop here: everything is
/// synchronous.
#[derive(Default)]
pub struct ResourceCollector {
    resources: Mutex<Vec<Resource>>,
}

impl ResourceCollector {
    pub fn drain(&self) -> Vec<Resource> {
        std::mem::take(&mut *self.resources.lock().unwrap())
    }
}

impl NetCallback<Resource> for ResourceCollector {
    fn call(&self, _doc_id: usize, result: Result<Resource, Option<String>>) {
        if let Ok(r) = result {
            self.resources.lock().unwrap().push(r);
        }
    }
}

pub struct DataUriProvider {
    collector: Arc<ResourceCollector>,
}

impl DataUriProvider {
    pub fn new(collector: Arc<ResourceCollector>) -> Self {
        Self { collector }
    }
}

impl NetProvider<Resource> for DataUriProvider {
    fn fetch(&self, doc_id: usize, request: Request, handler: BoxedHandler<Resource>) {
        let Some(bytes) = crate::resource::resolve_data_uri(request.url.as_str()) else {
            return;
        };
        handler.bytes(doc_id, Bytes::from(bytes), self.collector.clone());
    }
}
