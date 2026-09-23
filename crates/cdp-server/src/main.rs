//! CDP server. Runs no JavaScript and touches no network: every resource in
//! the HTML must be a data: URI.

use cdp_server::session::{Output, Session};
use futures_util::{SinkExt, StreamExt};

// Só no binário: uma lib não tem por que impor o alocador de quem a usa.
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let address = std::env::var("CDP_ADDR").unwrap_or_else(|_| "127.0.0.1:9222".to_string());

    // `cdp-server --health`: o HEALTHCHECK de dentro do container. A imagem é
    // distroless, sem curl nem shell, então o próprio binário faz a checagem.
    if std::env::args().nth(1).as_deref() == Some("--health") {
        std::process::exit(match check_health(&address) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("unhealthy: {e}");
                1
            }
        });
    }

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

async fn serve(mut stream: TcpStream) -> Result<(), Box<dyn std::error::Error>> {
    // Health check na mesma porta do CDP. O pedido é espiado, não lido, para
    // que uma conexão WebSocket chegue intacta ao handshake.
    let mut head = [0u8; 16];
    let n = stream.peek(&mut head).await?;
    if is_health_request(&head[..n]) {
        return answer_health(&mut stream).await;
    }

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

/// `GET /health` ou `HEAD /health`, com ou sem query string.
fn is_health_request(head: &[u8]) -> bool {
    let path = head
        .strip_prefix(b"GET ")
        .or_else(|| head.strip_prefix(b"HEAD "));
    path.and_then(|p| p.strip_prefix(b"/health"))
        .is_some_and(|rest| matches!(rest.first(), Some(b' ' | b'?')))
}

async fn answer_health(stream: &mut TcpStream) -> Result<(), Box<dyn std::error::Error>> {
    // Consome o cabeçalho inteiro antes de responder: fechar o socket com bytes
    // ainda não lidos faz o kernel mandar RST, e o cliente pode perder a resposta.
    let mut request = Vec::with_capacity(512);
    let mut chunk = [0u8; 512];
    while !request.windows(4).any(|w| w == b"\r\n\r\n") && request.len() < 8192 {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..n]);
    }

    let body = if request.starts_with(b"HEAD ") {
        ""
    } else {
        "ok"
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{body}"
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await?;
    Ok(())
}

/// Chama o `/health` do servidor que escuta em `address`. Um servidor em
/// `0.0.0.0`/`[::]` é alcançado pelo loopback da mesma família.
fn check_health(address: &str) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::{Read, Write};
    use std::net::{SocketAddr, ToSocketAddrs};
    use std::time::Duration;

    let target = match address.parse::<SocketAddr>() {
        Ok(addr) => loopback_if_unspecified(addr),
        Err(_) => address
            .to_socket_addrs()?
            .next()
            .ok_or("CDP_ADDR sem endereço")?,
    };
    let timeout = Duration::from_secs(2);
    let mut stream = std::net::TcpStream::connect_timeout(&target, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    stream.write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;

    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    if response.starts_with(b"HTTP/1.1 200 ") {
        Ok(())
    } else {
        Err(format!(
            "resposta inesperada: {}",
            String::from_utf8_lossy(&response)
        )
        .into())
    }
}

fn loopback_if_unspecified(mut addr: std::net::SocketAddr) -> std::net::SocketAddr {
    use std::net::{Ipv4Addr, Ipv6Addr};
    if addr.ip().is_unspecified() {
        addr.set_ip(if addr.is_ipv4() {
            Ipv4Addr::LOCALHOST.into()
        } else {
            Ipv6Addr::LOCALHOST.into()
        });
    }
    addr
}

#[cfg(test)]
mod tests {
    use super::{is_health_request, loopback_if_unspecified};

    #[test]
    fn health_check_reaches_an_unspecified_bind_through_loopback() {
        let v4 = loopback_if_unspecified("0.0.0.0:9222".parse().unwrap());
        assert_eq!(v4, "127.0.0.1:9222".parse().unwrap());
        let v6 = loopback_if_unspecified("[::]:9333".parse().unwrap());
        assert_eq!(v6, "[::1]:9333".parse().unwrap());
        let fixed = loopback_if_unspecified("10.0.0.5:9222".parse().unwrap());
        assert_eq!(fixed, "10.0.0.5:9222".parse().unwrap());
    }

    #[test]
    fn recognizes_health_requests() {
        assert!(is_health_request(b"GET /health HTTP/1.1"));
        assert!(is_health_request(b"HEAD /health HTTP/1"));
        assert!(is_health_request(b"GET /health?x=1 HT"));
    }

    #[test]
    fn leaves_everything_else_to_the_websocket() {
        assert!(!is_health_request(b"GET / HTTP/1.1\r\n"));
        assert!(!is_health_request(b"GET /healthz HTTP/"));
        assert!(!is_health_request(b"POST /health HTTP"));
        assert!(!is_health_request(b"GET /heal"));
        assert!(!is_health_request(b""));
    }
}
