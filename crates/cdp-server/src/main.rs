//! CDP server. Runs no JavaScript and touches no network: every resource in
//! the HTML must be a data: URI.

use cdp_server::session::{Output, Session};
use futures_util::{SinkExt, StreamExt};

// Só no binário: uma lib não tem por que impor o alocador de quem a usa.
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let address = std::env::var("CDP_ADDR").unwrap_or_else(|_| "127.0.0.1:9222".to_string());
    let listener = TcpListener::bind(&address).await?;
    println!("CDP listening on ws://{address}");

    while let Ok((stream, _)) = listener.accept().await {
        tokio::spawn(async move {
            if let Err(e) = serve(stream).await {
                eprintln!("session ended: {e}");
            }
        });
    }
    Ok(())
}

/// CDP_DEBUG=1 mirrors the traffic on stderr — that is what shows which command
/// Puppeteer stops advancing on.
fn debugging() -> bool {
    std::env::var("CDP_DEBUG").is_ok_and(|v| !v.is_empty() && v != "0")
}

async fn serve(stream: TcpStream) -> Result<(), Box<dyn std::error::Error>> {
    let ws = tokio_tungstenite::accept_async(stream).await?;
    let (mut sink, mut source) = ws.split();
    let mut session = Session::new();

    while let Some(msg) = source.next().await {
        let Message::Text(text) = msg? else { continue };
        if debugging() {
            eprintln!("<- {text}");
        }
        for output in session.handle(&text) {
            let payload = match output {
                Output::Response(t) => t,
                Output::Event(t) => t,
            };
            if debugging() {
                eprintln!("-> {payload}");
            }
            sink.send(Message::Text(payload)).await?;
        }
    }
    Ok(())
}
