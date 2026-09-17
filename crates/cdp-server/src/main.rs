//! Servidor CDP. Não executa JavaScript e não acessa a rede: todo recurso do
//! HTML precisa ser data: URI.

use cdp_server::session::{Saida, Session};
use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let endereco = std::env::var("CDP_ADDR").unwrap_or_else(|_| "127.0.0.1:9222".to_string());
    let listener = TcpListener::bind(&endereco).await?;
    println!("CDP escutando em ws://{endereco}");

    while let Ok((stream, _)) = listener.accept().await {
        tokio::spawn(async move {
            if let Err(e) = atender(stream).await {
                eprintln!("sessão encerrada: {e}");
            }
        });
    }
    Ok(())
}

async fn atender(stream: TcpStream) -> Result<(), Box<dyn std::error::Error>> {
    let ws = tokio_tungstenite::accept_async(stream).await?;
    let (mut envio, mut recepcao) = ws.split();
    let mut sessao = Session::new();

    while let Some(msg) = recepcao.next().await {
        let Message::Text(texto) = msg? else { continue };
        for saida in sessao.handle(&texto) {
            let payload = match saida {
                Saida::Resposta(t) => t,
                Saida::Evento(t) => t,
            };
            envio.send(Message::Text(payload.into())).await?;
        }
    }
    Ok(())
}
