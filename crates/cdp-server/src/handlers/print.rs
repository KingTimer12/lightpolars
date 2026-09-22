//! `Page.printToPDF` and the IO stream it hands the client.

use crate::session::{Command, Output, Session};
use base64::Engine as _;
use serde_json::json;

pub fn handle(session: &mut Session, cmd: &Command) -> Vec<Output> {
    match cmd.method.as_str() {
        "Page.printToPDF" => print_to_pdf(session, cmd),
        "IO.read" => {
            let handle = cmd.string("handle").unwrap_or("");
            let (data, eof) = session.streams.read(handle);
            vec![cmd.ok(json!({ "data": data, "base64Encoded": true, "eof": eof }))]
        }
        _ => {
            session.streams.close(cmd.string("handle").unwrap_or(""));
            vec![cmd.ok(json!({}))]
        }
    }
}

fn print_to_pdf(session: &mut Session, cmd: &Command) -> Vec<Output> {
    let bytes = build_pdf(session, cmd);

    // Puppeteer asks for ReturnAsStream and then drains it with IO.read; it
    // asserts `result.stream`, so answering with `data` alone breaks it.
    if cmd.string("transferMode") == Some("ReturnAsStream") {
        let handle = session.streams.open(bytes);
        vec![cmd.ok(json!({ "stream": handle }))]
    } else {
        let data = base64::engine::general_purpose::STANDARD.encode(bytes);
        vec![cmd.ok(json!({ "data": data }))]
    }
}

fn build_pdf(session: &mut Session, cmd: &Command) -> Vec<u8> {
    let geo = crate::print_params::geometry_from_print_params(&cmd.params);
    let dl = session.page_mut(&cmd.session).layout(geo.content_width());
    let pages = paginate::paginate(&dl, None, None, &geo);
    pdf_out::render_pdf(&pages, &dl.fonts, &geo)
}
