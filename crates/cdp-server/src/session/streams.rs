//! Byte streams handed out by `transferMode: ReturnAsStream` and drained by
//! `IO.read`.

use base64::Engine as _;
use std::collections::HashMap;

/// Size of each chunk returned by IO.read.
const CHUNK: usize = 1 << 16;

/// A stream keeps its bytes only until they have all been handed out. The
/// handle stays alive after that, because the client still has an `IO.close`
/// to send and an unknown handle would be answered differently.
enum Stream {
    Open { bytes: Vec<u8>, pos: usize },
    Drained,
}

#[derive(Default)]
pub struct StreamStore {
    counter: u32,
    open: HashMap<String, Stream>,
}

impl StreamStore {
    /// Stores the bytes and returns the handle the client will read from.
    pub fn open(&mut self, bytes: Vec<u8>) -> String {
        self.counter += 1;
        let handle = format!("stream-{}", self.counter);
        self.open.insert(handle.clone(), Stream::Open { bytes, pos: 0 });
        handle
    }

    /// The next base64 chunk plus whether the stream is exhausted. An unknown
    /// handle reads as an immediate EOF rather than an error.
    ///
    /// Reaching the end frees the buffer immediately instead of waiting for
    /// `IO.close`: a whole PDF stays resident otherwise, and a client that
    /// drops the connection mid-drain would hold it until the session dies.
    pub fn read(&mut self, handle: &str) -> (String, bool) {
        let Some(Stream::Open { bytes, pos }) = self.open.get_mut(handle) else {
            return (String::new(), true);
        };
        let end = (*pos + CHUNK).min(bytes.len());
        let chunk = base64::engine::general_purpose::STANDARD.encode(&bytes[*pos..end]);
        *pos = end;
        let eof = end >= bytes.len();
        if eof {
            self.open.insert(handle.to_string(), Stream::Drained);
        }
        (chunk, eof)
    }

    pub fn close(&mut self, handle: &str) {
        self.open.remove(handle);
    }
}
