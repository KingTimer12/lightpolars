//! Byte streams handed out by `transferMode: ReturnAsStream` and drained by
//! `IO.read`.

use base64::Engine as _;
use std::collections::HashMap;

/// Size of each chunk returned by IO.read.
const CHUNK: usize = 1 << 16;

#[derive(Default)]
pub struct StreamStore {
    counter: u32,
    open: HashMap<String, (Vec<u8>, usize)>,
}

impl StreamStore {
    /// Stores the bytes and returns the handle the client will read from.
    pub fn open(&mut self, bytes: Vec<u8>) -> String {
        self.counter += 1;
        let handle = format!("stream-{}", self.counter);
        self.open.insert(handle.clone(), (bytes, 0));
        handle
    }

    /// The next base64 chunk plus whether the stream is exhausted. An unknown
    /// handle reads as an immediate EOF rather than an error.
    pub fn read(&mut self, handle: &str) -> (String, bool) {
        let Some((bytes, pos)) = self.open.get_mut(handle) else {
            return (String::new(), true);
        };
        let end = (*pos + CHUNK).min(bytes.len());
        let chunk = base64::engine::general_purpose::STANDARD.encode(&bytes[*pos..end]);
        *pos = end;
        (chunk, end >= bytes.len())
    }

    pub fn close(&mut self, handle: &str) {
        self.open.remove(handle);
    }
}
