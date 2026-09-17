//! Máquina de estados do CDP, independente de transporte.
//! O Puppeteer bloqueia num LifecycleWatcher depois de setDocumentContent:
//! sem os eventos de ciclo de vida, setContent trava até o timeout.

use base64::Engine as _;
use paginate::paginate;
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq)]
pub enum Saida {
    Resposta(String),
    Evento(String),
}

#[derive(Default)]
pub struct Session {
    html: Option<String>,
    frame_id: String,
    contador_target: u32,
}

impl Session {
    pub fn new() -> Self {
        Self { frame_id: "frame-1".to_string(), ..Default::default() }
    }

    pub fn handle(&mut self, msg: &str) -> Vec<Saida> {
        let Ok(v) = serde_json::from_str::<Value>(msg) else {
            return Vec::new();
        };
        let id = v.get("id").and_then(Value::as_i64).unwrap_or(0);
        let metodo = v.get("method").and_then(Value::as_str).unwrap_or("");
        let params = v.get("params").cloned().unwrap_or_else(|| json!({}));

        match metodo {
            "Target.createTarget" => {
                self.contador_target += 1;
                let tid = format!("target-{}", self.contador_target);
                vec![self.ok(id, json!({ "targetId": tid }))]
            }
            "Target.attachToTarget" => {
                vec![self.ok(id, json!({ "sessionId": "session-1" }))]
            }
            "Page.setDocumentContent" => {
                self.html = params
                    .get("html")
                    .and_then(Value::as_str)
                    .map(str::to_string);

                let mut saidas = vec![self.ok(id, json!({}))];
                for nome in ["init", "load", "DOMContentLoaded", "networkIdle"] {
                    saidas.push(self.lifecycle(nome));
                }
                saidas
            }
            "Page.printToPDF" => {
                let bytes = self.gerar_pdf(&params);
                let dados = base64::engine::general_purpose::STANDARD.encode(bytes);
                vec![self.ok(id, json!({ "data": dados }))]
            }
            // Aceites sem efeito: nada sai para a rede, nenhum script roda.
            _ => vec![self.ok(id, json!({}))],
        }
    }

    fn gerar_pdf(&self, params: &Value) -> Vec<u8> {
        let geo = crate::params::geometry_from_print_params(params);
        let html = self.html.clone().unwrap_or_default();
        let dl = render_core::render_html(&html, geo.content_width());
        let pages = paginate(&dl, None, None, &geo);
        pdf_out::render_pdf(&pages, &dl.fonts, &geo)
    }

    fn ok(&self, id: i64, result: Value) -> Saida {
        Saida::Resposta(json!({ "id": id, "result": result }).to_string())
    }

    fn lifecycle(&self, nome: &str) -> Saida {
        Saida::Evento(
            json!({
                "method": "Page.lifecycleEvent",
                "params": {
                    "frameId": self.frame_id,
                    "loaderId": "loader-1",
                    "name": nome,
                    "timestamp": 0.0
                }
            })
            .to_string(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn json_de(s: &str) -> Value {
        serde_json::from_str(s).expect("saída não é JSON válido")
    }

    fn respostas(saidas: &[Saida]) -> Vec<Value> {
        saidas
            .iter()
            .filter_map(|s| match s {
                Saida::Resposta(t) => Some(json_de(t)),
                _ => None,
            })
            .collect()
    }

    fn eventos(saidas: &[Saida]) -> Vec<Value> {
        saidas
            .iter()
            .filter_map(|s| match s {
                Saida::Evento(t) => Some(json_de(t)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn responde_target_create_com_um_target_id() {
        let mut s = Session::new();
        let out = s.handle(r#"{"id":1,"method":"Target.createTarget","params":{"url":"about:blank"}}"#);
        let r = respostas(&out);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0]["id"], 1);
        assert!(r[0]["result"]["targetId"].is_string());
    }

    #[test]
    fn metodo_desconhecido_responde_em_vez_de_travar() {
        let mut s = Session::new();
        let out = s.handle(r#"{"id":7,"method":"Inexistente.metodo","params":{}}"#);
        let r = respostas(&out);
        assert_eq!(r.len(), 1, "todo comando precisa de exatamente uma resposta");
        assert_eq!(r[0]["id"], 7);
    }

    #[test]
    fn set_document_content_emite_init_load_e_network_idle() {
        let mut s = Session::new();
        s.handle(r#"{"id":1,"method":"Page.enable","params":{}}"#);
        let out = s.handle(
            r#"{"id":2,"method":"Page.setDocumentContent","params":{"frameId":"f1","html":"<p>oi</p>"}}"#,
        );

        let nomes: Vec<String> = eventos(&out)
            .iter()
            .filter(|e| e["method"] == "Page.lifecycleEvent")
            .map(|e| e["params"]["name"].as_str().unwrap_or_default().to_string())
            .collect();

        assert!(nomes.contains(&"init".to_string()), "faltou init: {nomes:?}");
        assert!(nomes.contains(&"load".to_string()), "faltou load: {nomes:?}");
        assert!(
            nomes.contains(&"networkIdle".to_string()),
            "faltou networkIdle — rh/_funcionarios.js usa waitUntil networkidle0: {nomes:?}"
        );
        assert_eq!(respostas(&out).len(), 1);
    }

    #[test]
    fn print_to_pdf_devolve_base64_de_um_pdf() {
        use base64::Engine as _;
        let mut s = Session::new();
        s.handle(r#"{"id":1,"method":"Page.setDocumentContent","params":{"frameId":"f1","html":"<p>Conteudo</p>"}}"#);
        let out = s.handle(
            r#"{"id":2,"method":"Page.printToPDF","params":{"paperWidth":8.27,"paperHeight":11.7,"marginTop":0,"marginRight":0,"marginBottom":0,"marginLeft":0,"printBackground":true}}"#,
        );
        let r = respostas(&out);
        let dados = r[0]["result"]["data"].as_str().expect("sem campo data");
        let bytes = base64::engine::general_purpose::STANDARD.decode(dados).unwrap();
        assert!(bytes.starts_with(b"%PDF-"));
    }

    #[test]
    fn print_to_pdf_sem_conteudo_previo_nao_panica() {
        let mut s = Session::new();
        let out = s.handle(r#"{"id":1,"method":"Page.printToPDF","params":{}}"#);
        assert_eq!(respostas(&out).len(), 1);
    }
}
