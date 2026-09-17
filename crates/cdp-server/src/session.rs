//! Máquina de estados do CDP, independente de transporte.
//! O Puppeteer bloqueia num LifecycleWatcher depois de setDocumentContent:
//! sem os eventos de ciclo de vida, setContent trava até o timeout.

use base64::Engine as _;
use paginate::paginate;
use serde_json::{Value, json};
use std::collections::HashMap;

/// Tamanho de cada pedaço devolvido por IO.read.
const CHUNK: usize = 1 << 16;

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
    contador_contexto: i64,
    contador_stream: u32,
    /// PDFs entregues em transferMode ReturnAsStream, consumidos por IO.read.
    streams: HashMap<String, (Vec<u8>, usize)>,
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
                // O Puppeteer não chama attachToTarget: ele liga Target.setAutoAttach
                // e espera o attachedToTarget nascer junto com o target. Sem ele,
                // Browser.waitForTarget fica pendurado até o protocolTimeout.
                vec![
                    self.evento_bruto(
                        &sessao,
                        "Target.targetCreated",
                        json!({ "targetInfo": Self::target_info(&tid) }),
                    ),
                    self.ok(id, &sessao, json!({ "targetId": &tid })),
                    self.evento_bruto(
                        &sessao,
                        "Target.attachedToTarget",
                        json!({
                            "sessionId": SESSION_ID,
                            "targetInfo": Self::target_info(&tid),
                            "waitingForDebugger": false
                        }),
                    ),
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
            "Page.getFrameTree" => vec![self.ok(
                id,
                &sessao,
                json!({ "frameTree": { "frame": self.frame_info(), "childFrames": [] } }),
            )],
            "Page.getNavigationHistory" => vec![self.ok(
                id,
                &sessao,
                json!({
                    "currentIndex": 0,
                    "entries": [{
                        "id": 1,
                        "url": "about:blank",
                        "userTypedURL": "about:blank",
                        "title": "",
                        "transitionType": "typed"
                    }]
                }),
            )],
            "Target.getTargetInfo" => {
                let tid = self.ultimo_target.clone().unwrap_or_else(|| "target-1".to_string());
                vec![self.ok(id, &sessao, json!({ "targetInfo": Self::target_info(&tid) }))]
            }
            "Page.printToPDF" => {
                let bytes = self.gerar_pdf(&params);
                // O Puppeteer pede ReturnAsStream e depois drena com IO.read;
                // ele afirma `result.stream`, então devolver só `data` quebra.
                if params.get("transferMode").and_then(Value::as_str) == Some("ReturnAsStream") {
                    self.contador_stream += 1;
                    let handle = format!("stream-{}", self.contador_stream);
                    self.streams.insert(handle.clone(), (bytes, 0));
                    vec![self.ok(id, &sessao, json!({ "stream": handle }))]
                } else {
                    let dados = base64::engine::general_purpose::STANDARD.encode(bytes);
                    vec![self.ok(id, &sessao, json!({ "data": dados }))]
                }
            }
            "IO.read" => {
                let handle = params.get("handle").and_then(Value::as_str).unwrap_or("");
                let (dados, eof) = self.ler_stream(handle);
                vec![self.ok(
                    id,
                    &sessao,
                    json!({ "data": dados, "base64Encoded": true, "eof": eof }),
                )]
            }
            "IO.close" => {
                let handle = params.get("handle").and_then(Value::as_str).unwrap_or("");
                self.streams.remove(handle);
                vec![self.ok(id, &sessao, json!({}))]
            }
            "Runtime.enable" => {
                // Contexto do mundo principal: sem ele o FrameManager nunca liga
                // o frame a um realm e qualquer evaluate fica pendurado.
                self.contador_contexto += 1;
                let ctx = self.contador_contexto;
                vec![
                    self.evento_bruto(
                        &sessao,
                        "Runtime.executionContextCreated",
                        json!({ "context": self.contexto(ctx, "", true) }),
                    ),
                    self.ok(id, &sessao, json!({})),
                ]
            }
            "Page.createIsolatedWorld" => {
                self.contador_contexto += 1;
                let ctx = self.contador_contexto;
                let nome = params
                    .get("worldName")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                vec![
                    self.evento_bruto(
                        &sessao,
                        "Runtime.executionContextCreated",
                        json!({ "context": self.contexto(ctx, &nome, false) }),
                    ),
                    self.ok(id, &sessao, json!({ "executionContextId": ctx })),
                ]
            }
            "Page.addScriptToEvaluateOnNewDocument" => {
                vec![self.ok(id, &sessao, json!({ "identifier": "1" }))]
            }
            // Não há motor de JavaScript: toda avaliação devolve undefined. O
            // único uso no caminho do PDF é `document.fonts.ready`, e as fontes
            // já são resolvidas de forma síncrona durante o layout.
            "Runtime.evaluate" | "Runtime.callFunctionOn" => {
                vec![self.ok(id, &sessao, json!({ "result": { "type": "undefined" } }))]
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

    fn ler_stream(&mut self, handle: &str) -> (String, bool) {
        let Some((bytes, pos)) = self.streams.get_mut(handle) else {
            return (String::new(), true);
        };
        let fim = (*pos + CHUNK).min(bytes.len());
        let pedaco = base64::engine::general_purpose::STANDARD.encode(&bytes[*pos..fim]);
        *pos = fim;
        (pedaco, fim >= bytes.len())
    }

    fn contexto(&self, id: i64, nome: &str, principal: bool) -> Value {
        json!({
            "id": id,
            "origin": "://",
            "name": nome,
            "uniqueId": format!("ctx-{id}"),
            "auxData": {
                "frameId": self.frame_id,
                "isDefault": principal,
                "type": if principal { "default" } else { "isolated" }
            }
        })
    }

    fn frame_info(&self) -> Value {
        json!({
            "id": self.frame_id,
            "loaderId": "loader-1",
            "url": "about:blank",
            "domainAndRegistry": "",
            "securityOrigin": "://",
            "mimeType": "text/html",
            "secureContextType": "Secure",
            "crossOriginIsolatedContextType": "NotIsolated",
            "gatedAPIFeatures": []
        })
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
    #[test]
    fn create_target_emite_attached_sozinho_para_o_auto_attach() {
        // O Puppeteer liga Target.setAutoAttach e nunca chama attachToTarget:
        // sem este evento, Browser.waitForTarget fica pendurado até o timeout.
        let mut s = Session::new();
        let out = s.handle(r#"{"id":1,"method":"Target.createTarget","params":{"url":"about:blank"}}"#);
        let evs = eventos(&out);
        let anexado = evs
            .iter()
            .find(|e| e["method"] == "Target.attachedToTarget")
            .expect("faltou Target.attachedToTarget");
        assert_eq!(anexado["params"]["sessionId"], "session-1");
        assert_eq!(anexado["params"]["targetInfo"]["targetId"], "target-1");
        assert_eq!(anexado["params"]["targetInfo"]["type"], "page");
    }

    #[test]
    fn get_frame_tree_devolve_um_frame_de_verdade() {
        // Resposta vazia fazia o FrameManager estourar em `frameTree.frame`.
        let mut s = Session::new();
        let out = s.handle(r#"{"id":1,"method":"Page.getFrameTree","params":{}}"#);
        let r = respostas(&out);
        let frame = &r[0]["result"]["frameTree"]["frame"];
        assert_eq!(frame["id"], "frame-1");
        assert!(frame["loaderId"].is_string());
        assert_eq!(frame["mimeType"], "text/html");
        assert!(r[0]["result"]["frameTree"]["childFrames"].is_array());
    }

    #[test]
    fn runtime_enable_publica_o_contexto_do_mundo_principal() {
        let mut s = Session::new();
        let out = s.handle(r#"{"id":1,"method":"Runtime.enable","params":{}}"#);
        let e = &eventos(&out)[0];
        assert_eq!(e["method"], "Runtime.executionContextCreated");
        assert_eq!(e["params"]["context"]["auxData"]["isDefault"], true);
        assert_eq!(e["params"]["context"]["auxData"]["frameId"], "frame-1");
    }

    #[test]
    fn isolated_world_publica_contexto_com_o_nome_pedido() {
        let mut s = Session::new();
        let out = s.handle(
            r#"{"id":1,"method":"Page.createIsolatedWorld","params":{"frameId":"frame-1","worldName":"__puppeteer_utility_world__x"}}"#,
        );
        let e = &eventos(&out)[0];
        assert_eq!(e["params"]["context"]["name"], "__puppeteer_utility_world__x");
        assert_eq!(e["params"]["context"]["auxData"]["isDefault"], false);
        let ctx = e["params"]["context"]["id"].as_i64().unwrap();
        assert_eq!(respostas(&out)[0]["result"]["executionContextId"], ctx);
    }

    #[test]
    fn evaluate_devolve_undefined_por_nao_haver_motor_de_js() {
        let mut s = Session::new();
        let out = s.handle(r#"{"id":1,"method":"Runtime.callFunctionOn","params":{"functionDeclaration":"() => document.fonts.ready"}}"#);
        assert_eq!(respostas(&out)[0]["result"]["result"]["type"], "undefined");
    }

    #[test]
    fn print_to_pdf_em_stream_e_drenado_por_io_read() {
        use base64::Engine as _;
        let mut s = Session::new();
        s.handle(r#"{"id":1,"method":"Page.setDocumentContent","params":{"frameId":"f1","html":"<p>Conteudo</p>"}}"#);
        let out = s.handle(
            r#"{"id":2,"method":"Page.printToPDF","params":{"transferMode":"ReturnAsStream","paperWidth":8.27,"paperHeight":11.7}}"#,
        );
        let handle = respostas(&out)[0]["result"]["stream"]
            .as_str()
            .expect("sem handle de stream")
            .to_string();

        let mut bytes = Vec::new();
        let mut voltas = 0;
        loop {
            let out = s.handle(&format!(
                r#"{{"id":3,"method":"IO.read","params":{{"handle":"{handle}"}}}}"#
            ));
            let r = &respostas(&out)[0]["result"];
            assert_eq!(r["base64Encoded"], true);
            bytes.extend(
                base64::engine::general_purpose::STANDARD
                    .decode(r["data"].as_str().unwrap())
                    .unwrap(),
            );
            voltas += 1;
            if r["eof"].as_bool().unwrap_or(false) || voltas > 1000 {
                break;
            }
        }
        assert!(bytes.starts_with(b"%PDF-"));
        assert!(bytes.windows(5).any(|w| w == b"%%EOF"));

        let out = s.handle(&format!(
            r#"{{"id":4,"method":"IO.close","params":{{"handle":"{handle}"}}}}"#
        ));
        assert_eq!(respostas(&out).len(), 1);
    }

    #[test]
    fn io_read_de_handle_desconhecido_termina_em_vez_de_panicar() {
        let mut s = Session::new();
        let out = s.handle(r#"{"id":1,"method":"IO.read","params":{"handle":"nao-existe"}}"#);
        assert_eq!(respostas(&out)[0]["result"]["eof"], true);
    }
}
