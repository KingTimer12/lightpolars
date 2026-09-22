//! Heap profile of one heavy document, to attribute the peak RSS.
//!
//! Drives the session in process, exactly like `workload`, but under dhat so
//! every live byte at the peak carries the call stack that allocated it. The
//! document is the `texto-pesado` of `tests/bench`, which is the one that
//! drives the peak (800x4829 px).
//!
//!   cargo run --profile profiling --features dhat-heap --example heap
//!
//! Writes `dhat-heap.json` in the working directory; summarize it with
//! `tests/bench/heap_resumo.js`.

use cdp_server::session::{Output, Session};
use serde_json::json;

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

fn main() {
    let _profiler = dhat::Profiler::new_heap();

    let html = text_heavy(60);
    let mut session = Session::new();
    send(&mut session, "Page.setDocumentContent", json!({ "html": html }));

    // The screenshot is what peaks: layout of the whole document plus a pixmap
    // of width x height x 4 plus the encoder.
    let shot = send(
        &mut session,
        "Page.captureScreenshot",
        json!({ "format": "png", "captureBeyondViewport": true }),
    );
    let png = shot.get("data").and_then(|d| d.as_str()).map_or(0, str::len);

    let started = send(
        &mut session,
        "Page.printToPDF",
        json!({ "transferMode": "ReturnAsStream", "printBackground": true }),
    );
    let pdf = started.get("stream").is_some();

    println!("heap: png {png} bytes base64, pdf stream {pdf}");
}

fn send(session: &mut Session, method: &str, params: serde_json::Value) -> serde_json::Value {
    let msg = json!({ "id": 1, "method": method, "params": params }).to_string();
    for out in session.handle(&msg) {
        let Output::Response(text) = out else { continue };
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or(json!({}));
        if let Some(result) = parsed.get("result") {
            return result.clone();
        }
    }
    json!({})
}

/// Same document as `tests/bench/documentos.js` and `workload.rs`.
fn text_heavy(paragraphs: usize) -> String {
    let mut html = String::from(
        "<!DOCTYPE html><html><body style='font-family:sans-serif;font-size:12px'>",
    );
    for i in 0..paragraphs {
        html.push_str(&format!(
            "<h2 style='color:#334'>Seção {i}</h2><p>Texto de exemplo com acentuação, \
             números 1234567890 e pontuação — repetido para dar volume ao documento \
             e forçar a paginação a cortar em pontos diferentes.</p>"
        ));
    }
    html.push_str("</body></html>");
    html
}
