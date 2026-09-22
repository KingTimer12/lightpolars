//! Profiling workload for PGO/BOLT.
//!
//! Drives `Session` in process, with no socket and no Node, so the instrumented
//! build can be exercised inside a container with nothing else installed. The
//! CDP transport is a handful of lines in `main.rs` and never shows up hot; what
//! matters for the profile is layout, pagination and the two encoders, and all
//! of that is reached from here.
//!
//! Usage: `cargo run --example workload [iterations]`

use cdp_server::session::{Output, Session};
use serde_json::json;

// O mesmo alocador do binário: o perfil de PGO precisa ver o caminho de
// alocação que roda em produção, senão otimiza o malloc errado.
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;


fn main() {
    let iterations: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(20);

    let documents = [text_heavy(60), with_graphics(), tables(40)];
    let mut bytes = 0usize;

    for _ in 0..iterations {
        for html in &documents {
            let mut session = Session::new();
            send(&mut session, "Page.setDocumentContent", json!({ "html": html }));

            // Both output paths, because they share layout but diverge at the
            // encoder: printpdf on one side, tiny-skia plus swash on the other.
            bytes += drain_pdf(&mut session);
            bytes += screenshot(&mut session, "png");
            bytes += screenshot(&mut session, "jpeg");
        }
    }

    println!("workload done: {iterations} iterations, {bytes} bytes produced");
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

fn drain_pdf(session: &mut Session) -> usize {
    let started = send(
        session,
        "Page.printToPDF",
        json!({ "transferMode": "ReturnAsStream", "printBackground": true }),
    );
    let Some(handle) = started.get("stream").and_then(|s| s.as_str()).map(String::from) else {
        return 0;
    };

    let mut total = 0;
    loop {
        let chunk = send(session, "IO.read", json!({ "handle": handle }));
        total += chunk.get("data").and_then(|d| d.as_str()).map_or(0, str::len);
        if chunk.get("eof").and_then(|e| e.as_bool()).unwrap_or(true) {
            break;
        }
    }
    send(session, "IO.close", json!({ "handle": handle }));
    total
}

fn screenshot(session: &mut Session, format: &str) -> usize {
    let shot = send(
        session,
        "Page.captureScreenshot",
        json!({ "format": format, "captureBeyondViewport": true }),
    );
    shot.get("data").and_then(|d| d.as_str()).map_or(0, str::len)
}

/// Many short paragraphs: the case that dominates real invoices and reports,
/// and the one that spends its time in shaping and pagination.
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

/// Inline SVG plus a raster data URI, so the rewrite, usvg, resvg and the image
/// decoders all get profiled.
fn with_graphics() -> String {
    // 1x1 opaque red PNG.
    const PNG: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
    let mut html = String::from("<!DOCTYPE html><html><body>");
    for i in 0..20 {
        html.push_str(&format!(
            "<div style='background:#eef;padding:8px;margin:4px'>\
             <svg width='120' height='60' viewBox='0 0 120 60'>\
             <rect x='0' y='0' width='120' height='60' fill='#4a90d9'/>\
             <circle cx='{}' cy='30' r='20' fill='#ffcc00' opacity='0.7'/>\
             <path d='M10 50 L60 10 L110 50 Z' fill='none' stroke='#fff' stroke-width='3'/>\
             </svg>\
             <img src='{PNG}' style='width:80px;height:40px'>\
             <p>Bloco gráfico {i}</p></div>",
            20 + i * 4
        ));
    }
    html.push_str("</body></html>");
    html
}

/// Wide tables: lots of boxes and borders, which is the box-drawing path rather
/// than the text one.
fn tables(rows: usize) -> String {
    let mut html = String::from(
        "<!DOCTYPE html><html><body><table style='width:100%;border-collapse:collapse'>",
    );
    for r in 0..rows {
        html.push_str("<tr>");
        for c in 0..6 {
            html.push_str(&format!(
                "<td style='border:1px solid #999;padding:4px;background:{}'>{r}.{c}</td>",
                if r % 2 == 0 { "#fff" } else { "#f4f4f4" }
            ));
        }
        html.push_str("</tr>");
    }
    html.push_str("</table></body></html>");
    html
}
