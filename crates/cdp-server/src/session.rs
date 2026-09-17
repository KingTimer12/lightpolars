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

const SESSION_ID: &str = "session-1";

#[derive(Default)]
pub struct Session {
    html: Option<String>,
    frame_id: String,
    contador_target: u32,
    ultimo_target: Option<String>,
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
        // Modo flatten: toda mensagem de uma sessão anexada carrega sessionId, e
        // a resposta precisa devolver o mesmo campo ou o Puppeteer nunca a casa
        // com o comando e fica esperando.
        let sessao = v.get("sessionId").and_then(Value::as_str).map(str::to_string);

        match metodo {
            "Browser.getVersion" => vec![self.ok(
                id,
                &sessao,
                json!({
                    "protocolVersion": "1.3",
                    "product": "HeadlessChrome/0.0.0",
                    "revision": "0",
                    "userAgent": "cdp-server",
                    "jsVersion": "0"
                }),
            )],
            "Target.getBrowserContexts" => {
                vec![self.ok(id, &sessao, json!({ "browserContextIds": [] }))]
            }
            "Target.createTarget" => {
                self.contador_target += 1;
                let tid = format!("target-{}", self.contador_target);
                self.ultimo_target = Some(tid.clone());
                vec![
                    self.evento_bruto(
                        &sessao,
                        "Target.targetCreated",
                        json!({ "targetInfo": Self::target_info(&tid) }),
                    ),
                    self.ok(id, &sessao, json!({ "targetId": tid })),
                ]
            }
            "Target.attachToTarget" => {
                let tid = params
                    .get("targetId")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| self.ultimo_target.clone())
                    .unwrap_or_else(|| "target-1".to_string());
                vec![
                    // O evento vem antes da resposta: o Puppeteer registra a
                    // CDPSession ao vê-lo, e só então casa o result.
                    self.evento_bruto(
                        &sessao,
                        "Target.attachedToTarget",
                        json!({
                            "sessionId": SESSION_ID,
                            "targetInfo": Self::target_info(&tid),
                            "waitingForDebugger": false
                        }),
                    ),
                    self.ok(id, &sessao, json!({ "sessionId": SESSION_ID })),
                ]
            }
            "Page.setDocumentContent" => {
                self.html = params
                    .get("html")
                    .and_then(Value::as_str)
                    .map(str::to_string);

                // Os eventos saem antes da resposta: o LifecycleWatcher do
                // Puppeteer só começa a resolver depois de ver o ciclo completo,
                // e assim a ordem na rede não depende de corrida.
                let mut saidas: Vec<Saida> = ["init", "load", "DOMContentLoaded", "networkIdle"]
                    .into_iter()
                    .map(|nome| self.lifecycle(&sessao, nome))
                    .collect();
                saidas.push(self.ok(id, &sessao, json!({})));
                saidas
            }
            "Page.printToPDF" => {
                let bytes = self.gerar_pdf(&params);
                let dados = base64::engine::general_purpose::STANDARD.encode(bytes);
                vec![self.ok(id, &sessao, json!({ "data": dados }))]
            }
            // Aceites sem efeito: nada sai para a rede, nenhum script roda.
            _ => vec![self.ok(id, &sessao, json!({}))],
        }
    }

    fn gerar_pdf(&self, params: &Value) -> Vec<u8> {
        let geo = crate::params::geometry_from_print_params(params);
        let html = self.html.clone().unwrap_or_default();
        let dl = render_core::render_html(&html, geo.content_width());
        let pages = paginate(&dl, None, None, &geo);
        pdf_out::render_pdf(&pages, &dl.fonts, &geo)
    }

    fn target_info(target_id: &str) -> Value {
        json!({
            "targetId": target_id,
            "type": "page",
            "title": "",
            "url": "about:blank",
            "attached": true,
            "canAccessOpener": false,
            "browserContextId": "context-1"
        })
    }

    fn ok(&self, id: i64, sessao: &Option<String>, result: Value) -> Saida {
        let mut msg = json!({ "id": id, "result": result });
        Self::marcar_sessao(&mut msg, sessao);
        Saida::Resposta(msg.to_string())
    }

    fn evento_bruto(&self, sessao: &Option<String>, method: &str, params: Value) -> Saida {
        let mut msg = json!({ "method": method, "params": params });
        Self::marcar_sessao(&mut msg, sessao);
        Saida::Evento(msg.to_string())
    }

    fn marcar_sessao(msg: &mut Value, sessao: &Option<String>) {
        if let (Some(obj), Some(sid)) = (msg.as_object_mut(), sessao.as_ref()) {
            obj.insert("sessionId".into(), Value::String(sid.clone()));
        }
    }

    fn lifecycle(&self, sessao: &Option<String>, nome: &str) -> Saida {
        self.evento_bruto(
            sessao,
            "Page.lifecycleEvent",
            json!({
                "frameId": self.frame_id,
                "loaderId": "loader-1",
                "name": nome,
                "timestamp": 0.0
            }),
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
    #[test]
    fn attach_emite_o_evento_antes_da_resposta() {
        let mut s = Session::new();
        s.handle(r#"{"id":1,"method":"Target.createTarget","params":{"url":"about:blank"}}"#);
        let out = s.handle(r#"{"id":2,"method":"Target.attachToTarget","params":{"targetId":"target-1","flatten":true}}"#);

        // Ordem importa: o Puppeteer registra a CDPSession ao ver o evento.
        assert!(matches!(out[0], Saida::Evento(_)), "evento deve vir antes da resposta");
        let e = &eventos(&out)[0];
        assert_eq!(e["method"], "Target.attachedToTarget");
        assert_eq!(e["params"]["sessionId"], "session-1");
        assert_eq!(e["params"]["targetInfo"]["targetId"], "target-1");
        assert_eq!(e["params"]["targetInfo"]["type"], "page");

        let r = respostas(&out);
        assert_eq!(r[0]["result"]["sessionId"], "session-1");
    }

    #[test]
    fn session_id_do_comando_volta_na_resposta_e_nos_eventos() {
        let mut s = Session::new();
        let out = s.handle(
            r#"{"id":3,"sessionId":"session-1","method":"Page.setDocumentContent","params":{"frameId":"f1","html":"<p>oi</p>"}}"#,
        );
        for r in respostas(&out) {
            assert_eq!(r["sessionId"], "session-1", "resposta sem sessionId não casa com o comando");
        }
        for e in eventos(&out) {
            assert_eq!(e["sessionId"], "session-1", "evento de sessão precisa do sessionId");
        }
    }

    #[test]
    fn comando_sem_session_id_nao_ganha_o_campo() {
        let mut s = Session::new();
        let out = s.handle(r#"{"id":4,"method":"Target.getBrowserContexts","params":{}}"#);
        let r = respostas(&out);
        assert!(r[0].get("sessionId").is_none());
        assert!(r[0]["result"]["browserContextIds"].is_array());
    }

    #[test]
    fn ciclo_de_vida_sai_antes_da_resposta_de_set_document_content() {
        let mut s = Session::new();
        let out = s.handle(
            r#"{"id":5,"method":"Page.setDocumentContent","params":{"frameId":"f1","html":"<p>oi</p>"}}"#,
        );
        let ultima = out.last().expect("sem saída");
        assert!(matches!(ultima, Saida::Resposta(_)), "resposta deve fechar o lote");
    }
}
