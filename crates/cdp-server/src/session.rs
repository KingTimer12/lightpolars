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

/// Uma página aberta. A API do consumidor abre uma por requisição, então cada
/// uma precisa de sessionId e frameId próprios — reusar faz a segunda colidir
/// com a primeira do lado do Puppeteer.
#[derive(Debug, Clone)]
struct Pagina {
    target_id: String,
    frame_id: String,
    html: Option<String>,
    /// Viewport do screenshot, em px CSS. O PDF não usa: lá a largura vem da
    /// folha e das margens de `Page.printToPDF`.
    largura_viewport: f32,
    altura_viewport: f32,
    /// `deviceScaleFactor` do Emulation.setDeviceMetricsOverride.
    escala_dispositivo: f32,
}

/// Viewport padrão do Puppeteer quando nada é emulado.
const VIEWPORT_PADRAO: (f32, f32) = (800.0, 600.0);

impl Pagina {
    fn nova(target_id: String, frame_id: String) -> Self {
        Self {
            target_id,
            frame_id,
            html: None,
            largura_viewport: VIEWPORT_PADRAO.0,
            altura_viewport: VIEWPORT_PADRAO.1,
            escala_dispositivo: 1.0,
        }
    }
}

impl Default for Pagina {
    fn default() -> Self {
        Self::nova(String::new(), String::new())
    }
}

#[derive(Default)]
pub struct Session {
    contador_pagina: u32,
    contador_contexto: i64,
    contador_stream: u32,
    ultimo_target: Option<String>,
    /// Chaveado pelo sessionId que o Puppeteer usa em cada comando.
    paginas: HashMap<String, Pagina>,
    /// PDFs entregues em transferMode ReturnAsStream, consumidos por IO.read.
    streams: HashMap<String, (Vec<u8>, usize)>,
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    /// Página de um comando: a da sessão pedida, ou a implícita para quem fala
    /// CDP cru sem anexar sessão nenhuma.
    fn pagina_mut(&mut self, sessao: &Option<String>) -> &mut Pagina {
        let chave = sessao.clone().unwrap_or_else(|| "session-implicita".to_string());
        self.paginas
            .entry(chave)
            .or_insert_with(|| Pagina::nova("target-implicito".into(), "frame-1".into()))
    }

    fn pagina(&self, sessao: &Option<String>) -> Pagina {
        let chave = sessao.clone().unwrap_or_else(|| "session-implicita".to_string());
        self.paginas
            .get(&chave)
            .cloned()
            .unwrap_or_else(|| Pagina::nova("target-implicito".into(), "frame-1".into()))
    }

    #[cfg(test)]
    fn html_da_sessao(&self, sessao: &str) -> Option<&str> {
        self.paginas.get(sessao)?.html.as_deref()
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
                self.contador_pagina += 1;
                let n = self.contador_pagina;
                let tid = format!("target-{n}");
                let sid = format!("session-{n}");
                self.ultimo_target = Some(tid.clone());
                self.paginas.insert(
                    sid.clone(),
                    Pagina::nova(tid.clone(), format!("frame-{n}")),
                );
                // O Puppeteer não chama attachToTarget: liga Target.setAutoAttach
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
                            "sessionId": sid,
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
                let sid = self
                    .paginas
                    .iter()
                    .find(|(_, p)| p.target_id == tid)
                    .map(|(s, _)| s.clone())
                    .unwrap_or_else(|| "session-1".to_string());
                vec![
                    // O evento vem antes da resposta: o Puppeteer registra a
                    // CDPSession ao vê-lo, e só então casa o result.
                    self.evento_bruto(
                        &sessao,
                        "Target.attachedToTarget",
                        json!({
                            "sessionId": sid,
                            "targetInfo": Self::target_info(&tid),
                            "waitingForDebugger": false
                        }),
                    ),
                    self.ok(id, &sessao, json!({ "sessionId": sid })),
                ]
            }
            "Target.closeTarget" | "Target.detachFromTarget" | "Page.close" => {
                let tid = params
                    .get("targetId")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| self.pagina(&sessao).target_id);
                let sid = self
                    .paginas
                    .iter()
                    .find(|(_, p)| p.target_id == tid)
                    .map(|(s, _)| s.clone());
                if let Some(s) = &sid {
                    self.paginas.remove(s);
                }
                // page.close() espera _isClosedDeferred, que só resolve em
                // Target.detachedFromTarget. Sem esses dois eventos, close()
                // nunca retorna e a próxima página nunca é aberta.
                let mut saidas = vec![self.ok(id, &sessao, json!({ "success": true }))];
                if let Some(s) = sid {
                    saidas.push(self.evento_bruto(
                        &None,
                        "Target.detachedFromTarget",
                        json!({ "sessionId": s, "targetId": &tid }),
                    ));
                }
                saidas.push(self.evento_bruto(
                    &None,
                    "Target.targetDestroyed",
                    json!({ "targetId": tid }),
                ));
                saidas
            }
            "Page.setDocumentContent" => {
                let html = params
                    .get("html")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                self.pagina_mut(&sessao).html = html;

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
                json!({ "frameTree": { "frame": self.frame_info(&sessao), "childFrames": [] } }),
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
                let bytes = self.gerar_pdf(&sessao, &params);
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
            "Page.captureScreenshot" => match self.gerar_imagem(&sessao, &params) {
                Ok(bytes) => {
                    let dados = base64::engine::general_purpose::STANDARD.encode(bytes);
                    vec![self.ok(id, &sessao, json!({ "data": dados }))]
                }
                // Devolver um PNG onde o cliente pediu WebP seria pior que
                // falhar: ele gravaria o arquivo com a extensão errada.
                Err(motivo) => vec![self.erro(id, &sessao, &motivo)],
            },
            // O Puppeteer usa isto para montar o clip de um screenshot de página
            // inteira; devolver zeros faria a imagem sair vazia.
            "Page.getLayoutMetrics" => {
                let pagina = self.pagina(&sessao);
                let (largura, altura) = self.tamanho_do_conteudo(&pagina);
                let conteudo = json!({ "x": 0, "y": 0, "width": largura, "height": altura });
                let visual = json!({
                    "x": 0, "y": 0,
                    "width": pagina.largura_viewport,
                    "height": pagina.altura_viewport,
                    "clientWidth": pagina.largura_viewport,
                    "clientHeight": pagina.altura_viewport,
                    "pageX": 0, "pageY": 0, "scale": 1
                });
                vec![self.ok(
                    id,
                    &sessao,
                    json!({
                        "layoutViewport": visual,
                        "visualViewport": visual,
                        "contentSize": conteudo,
                        "cssLayoutViewport": visual,
                        "cssVisualViewport": visual,
                        "cssContentSize": conteudo
                    }),
                )]
            }
            "Emulation.setDeviceMetricsOverride" => {
                let numero = |chave: &str| params.get(chave).and_then(Value::as_f64);
                let pagina = self.pagina_mut(&sessao);
                // Largura/altura 0 significam "usa o padrão", não uma janela nula.
                if let Some(w) = numero("width").filter(|w| *w > 0.0) {
                    pagina.largura_viewport = w as f32;
                }
                if let Some(h) = numero("height").filter(|h| *h > 0.0) {
                    pagina.altura_viewport = h as f32;
                }
                if let Some(e) = numero("deviceScaleFactor").filter(|e| *e > 0.0) {
                    pagina.escala_dispositivo = e as f32;
                }
                vec![self.ok(id, &sessao, json!({}))]
            }
            "Emulation.clearDeviceMetricsOverride" => {
                let pagina = self.pagina_mut(&sessao);
                pagina.largura_viewport = VIEWPORT_PADRAO.0;
                pagina.altura_viewport = VIEWPORT_PADRAO.1;
                pagina.escala_dispositivo = 1.0;
                vec![self.ok(id, &sessao, json!({}))]
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
                        json!({ "context": self.contexto(&sessao, ctx, "", true) }),
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
                        json!({ "context": self.contexto(&sessao, ctx, &nome, false) }),
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

    fn gerar_pdf(&self, sessao: &Option<String>, params: &Value) -> Vec<u8> {
        let geo = crate::params::geometry_from_print_params(params);
        let html = self.pagina(sessao).html.unwrap_or_default();
        let dl = render_core::render_html(&html, geo.content_width());
        let pages = paginate(&dl, None, None, &geo);
        pdf_out::render_pdf(&pages, &dl.fonts, &geo)
    }

    /// Dimensões do documento renderizado na largura de viewport da página.
    fn tamanho_do_conteudo(&self, pagina: &Pagina) -> (f32, f32) {
        let html = pagina.html.clone().unwrap_or_default();
        let dl = render_core::render_html(&html, pagina.largura_viewport);
        (
            pagina.largura_viewport,
            dl.content_height().max(pagina.altura_viewport),
        )
    }

    /// Desenha a página como imagem. Devolve `Err` com a mensagem de erro do CDP
    /// quando o formato pedido não é suportado.
    fn gerar_imagem(&self, sessao: &Option<String>, params: &Value) -> Result<Vec<u8>, String> {
        let formato = params
            .get("format")
            .and_then(Value::as_str)
            .unwrap_or("png")
            .to_ascii_lowercase();
        if formato != "png" && formato != "jpeg" && formato != "jpg" {
            return Err(format!(
                "captureScreenshot: formato '{formato}' não suportado (use png ou jpeg)"
            ));
        }

        let pagina = self.pagina(sessao);
        let html = pagina.html.clone().unwrap_or_default();
        let dl = render_core::render_html(&html, pagina.largura_viewport);

        let clip = params.get("clip").filter(|c| c.is_object());
        let alem_do_viewport = params
            .get("captureBeyondViewport")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let (page, geo, escala) = match clip {
            Some(c) => {
                let num = |chave: &str, padrao: f64| c.get(chave).and_then(Value::as_f64).unwrap_or(padrao);
                let recorte = render_ir::Rect {
                    x: num("x", 0.0) as f32,
                    y: num("y", 0.0) as f32,
                    width: num("width", pagina.largura_viewport as f64) as f32,
                    height: num("height", pagina.altura_viewport as f64) as f32,
                };
                let (p, g) = paginate::recortar(&dl, recorte);
                // Com clip o Chromium usa `clip.scale` como fator final; o
                // deviceScaleFactor já está embutido nas medidas que o cliente
                // calculou a partir de getLayoutMetrics.
                (p, g, num("scale", 1.0) as f32)
            }
            None => {
                let (p, mut g) = paginate::pagina_unica(&dl);
                g.sheet_width = pagina.largura_viewport;
                // Sem captureBeyondViewport a imagem tem o tamanho da janela,
                // mesmo que o conteúdo seja menor (sobra fundo) ou maior (corta).
                g.sheet_height = if alem_do_viewport {
                    g.sheet_height.max(pagina.altura_viewport)
                } else {
                    pagina.altura_viewport
                };
                (p, g, pagina.escala_dispositivo)
            }
        };

        let bytes = if formato == "png" {
            raster_out::render_png(&page, &dl.fonts, &geo, escala)
        } else {
            let qualidade = params.get("quality").and_then(Value::as_u64).unwrap_or(80);
            raster_out::render_jpeg(&page, &dl.fonts, &geo, escala, qualidade.min(100) as u8)
        };
        bytes.ok_or_else(|| "captureScreenshot: geometria inválida para a imagem".to_string())
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

    fn contexto(&self, sessao: &Option<String>, id: i64, nome: &str, principal: bool) -> Value {
        json!({
            "id": id,
            "origin": "://",
            "name": nome,
            "uniqueId": format!("ctx-{id}"),
            "auxData": {
                "frameId": self.pagina(sessao).frame_id,
                "isDefault": principal,
                "type": if principal { "default" } else { "isolated" }
            }
        })
    }

    fn frame_info(&self, sessao: &Option<String>) -> Value {
        json!({
            "id": self.pagina(sessao).frame_id,
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

    /// Resposta de erro do CDP. `-32000` é o código de erro de servidor que o
    /// Puppeteer converte numa exceção no `await` do comando.
    fn erro(&self, id: i64, sessao: &Option<String>, mensagem: &str) -> Saida {
        let mut msg = json!({ "id": id, "error": { "code": -32000, "message": mensagem } });
        Self::marcar_sessao(&mut msg, sessao);
        Saida::Resposta(msg.to_string())
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
                "frameId": self.pagina(sessao).frame_id,
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
    #[test]
    fn close_target_avisa_que_a_pagina_morreu() {
        // page.close() espera _isClosedDeferred, que só resolve em
        // Target.detachedFromTarget. Sem ele, close() nunca retorna.
        let mut s = Session::new();
        s.handle(r#"{"id":1,"method":"Target.createTarget","params":{"url":"about:blank"}}"#);
        let out = s.handle(r#"{"id":2,"method":"Target.closeTarget","params":{"targetId":"target-1"}}"#);

        let evs = eventos(&out);
        let desanexado = evs
            .iter()
            .find(|e| e["method"] == "Target.detachedFromTarget")
            .expect("faltou Target.detachedFromTarget");
        assert_eq!(desanexado["params"]["sessionId"], "session-1");
        assert_eq!(desanexado["params"]["targetId"], "target-1");
        assert!(
            evs.iter().any(|e| e["method"] == "Target.targetDestroyed"),
            "faltou Target.targetDestroyed"
        );
        assert_eq!(respostas(&out).len(), 1);
    }

    #[test]
    fn paginas_em_sequencia_ganham_sessao_e_frame_proprios() {
        // A API abre uma página por requisição: reusar sessionId/frameId fazia a
        // segunda colidir com a primeira do lado do Puppeteer.
        let mut s = Session::new();
        let p1 = s.handle(r#"{"id":1,"method":"Target.createTarget","params":{"url":"about:blank"}}"#);
        let a1 = eventos(&p1)
            .into_iter()
            .find(|e| e["method"] == "Target.attachedToTarget")
            .unwrap();
        s.handle(r#"{"id":2,"method":"Target.closeTarget","params":{"targetId":"target-1"}}"#);

        let p2 = s.handle(r#"{"id":3,"method":"Target.createTarget","params":{"url":"about:blank"}}"#);
        let a2 = eventos(&p2)
            .into_iter()
            .find(|e| e["method"] == "Target.attachedToTarget")
            .unwrap();

        assert_ne!(a1["params"]["sessionId"], a2["params"]["sessionId"]);
        assert_ne!(
            a1["params"]["targetInfo"]["targetId"],
            a2["params"]["targetInfo"]["targetId"]
        );
    }

    #[test]
    fn cada_pagina_guarda_o_proprio_html() {
        let mut s = Session::new();
        s.handle(r#"{"id":1,"method":"Target.createTarget","params":{"url":"about:blank"}}"#);
        s.handle(r#"{"id":2,"method":"Target.createTarget","params":{"url":"about:blank"}}"#);

        s.handle(r#"{"id":3,"sessionId":"session-1","method":"Page.setDocumentContent","params":{"html":"<p>primeira</p>"}}"#);
        s.handle(r#"{"id":4,"sessionId":"session-2","method":"Page.setDocumentContent","params":{"html":"<p>segunda</p>"}}"#);

        assert_eq!(s.html_da_sessao("session-1"), Some("<p>primeira</p>"));
        assert_eq!(s.html_da_sessao("session-2"), Some("<p>segunda</p>"));
    }

    #[test]
    fn frame_tree_responde_o_frame_da_sessao_pedida() {
        let mut s = Session::new();
        s.handle(r#"{"id":1,"method":"Target.createTarget","params":{"url":"about:blank"}}"#);
        s.handle(r#"{"id":2,"method":"Target.createTarget","params":{"url":"about:blank"}}"#);

        let f1 = respostas(&s.handle(r#"{"id":3,"sessionId":"session-1","method":"Page.getFrameTree","params":{}}"#))[0]
            ["result"]["frameTree"]["frame"]["id"]
            .clone();
        let f2 = respostas(&s.handle(r#"{"id":4,"sessionId":"session-2","method":"Page.getFrameTree","params":{}}"#))[0]
            ["result"]["frameTree"]["frame"]["id"]
            .clone();
        assert_ne!(f1, f2, "frames de páginas diferentes não podem colidir");
    }

    // --- Page.captureScreenshot ---

    /// Define o HTML da sessão implícita e devolve a sessão pronta.
    fn com_html(html: &str) -> Session {
        let mut s = Session::new();
        let msg = json!({
            "id": 1,
            "method": "Page.setDocumentContent",
            "params": { "frameId": "f", "html": html }
        });
        s.handle(&msg.to_string());
        s
    }

    fn png_de(s: &mut Session, params: Value) -> Vec<u8> {
        let msg = json!({ "id": 9, "method": "Page.captureScreenshot", "params": params });
        let r = respostas(&s.handle(&msg.to_string()));
        assert_eq!(r.len(), 1);
        let dados = r[0]["result"]["data"]
            .as_str()
            .unwrap_or_else(|| panic!("sem data na resposta: {}", r[0]));
        base64::engine::general_purpose::STANDARD.decode(dados).unwrap()
    }

    /// Largura e altura lidas do cabeçalho IHDR do PNG (bytes 16..24).
    fn dimensoes_png(bytes: &[u8]) -> (u32, u32) {
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "não é PNG");
        let n = |i: usize| u32::from_be_bytes(bytes[i..i + 4].try_into().unwrap());
        (n(16), n(20))
    }

    #[test]
    fn screenshot_devolve_png_no_tamanho_do_viewport() {
        let mut s = com_html("<p>oi</p>");
        let bytes = png_de(&mut s, json!({}));
        assert_eq!(dimensoes_png(&bytes), (800, 600), "viewport padrão do Puppeteer");
    }

    #[test]
    fn capture_beyond_viewport_usa_a_altura_do_conteudo() {
        let mut s = com_html(r#"<div style="height:2000px"></div>"#);
        let bytes = png_de(&mut s, json!({ "captureBeyondViewport": true }));
        let (_, altura) = dimensoes_png(&bytes);
        assert!(altura >= 2000, "conteúdo de 2000px saiu com {altura}px");
    }

    #[test]
    fn clip_define_o_tamanho_e_a_escala_da_imagem() {
        let mut s = com_html("<p>oi</p>");
        let bytes = png_de(
            &mut s,
            json!({ "clip": { "x": 0, "y": 0, "width": 100, "height": 50, "scale": 2 } }),
        );
        assert_eq!(dimensoes_png(&bytes), (200, 100));
    }

    #[test]
    fn device_scale_factor_multiplica_o_screenshot() {
        let mut s = com_html("<p>oi</p>");
        s.handle(
            r#"{"id":2,"method":"Emulation.setDeviceMetricsOverride","params":{"width":400,"height":300,"deviceScaleFactor":2}}"#,
        );
        let bytes = png_de(&mut s, json!({}));
        assert_eq!(dimensoes_png(&bytes), (800, 600), "400x300 em 2x");
    }

    #[test]
    fn clear_device_metrics_volta_ao_viewport_padrao() {
        let mut s = com_html("<p>oi</p>");
        s.handle(
            r#"{"id":2,"method":"Emulation.setDeviceMetricsOverride","params":{"width":400,"height":300,"deviceScaleFactor":2}}"#,
        );
        s.handle(r#"{"id":3,"method":"Emulation.clearDeviceMetricsOverride","params":{}}"#);
        let bytes = png_de(&mut s, json!({}));
        assert_eq!(dimensoes_png(&bytes), (800, 600));
    }

    #[test]
    fn screenshot_em_jpeg_sai_com_marcador_de_jpeg() {
        let mut s = com_html("<p>oi</p>");
        let bytes = png_de(&mut s, json!({ "format": "jpeg", "quality": 60 }));
        assert_eq!(&bytes[..2], b"\xff\xd8", "SOI de JPEG");
    }

    #[test]
    fn formato_nao_suportado_vira_erro_em_vez_de_png_disfarcado() {
        let mut s = com_html("<p>oi</p>");
        let r = respostas(&s.handle(
            r#"{"id":9,"method":"Page.captureScreenshot","params":{"format":"webp"}}"#,
        ));
        assert!(r[0]["result"].is_null(), "não devia responder com sucesso");
        assert_eq!(r[0]["error"]["code"], -32000);
        assert!(
            r[0]["error"]["message"].as_str().unwrap().contains("webp"),
            "mensagem devia nomear o formato: {}",
            r[0]["error"]["message"]
        );
    }

    #[test]
    fn get_layout_metrics_reporta_a_altura_real_do_conteudo() {
        let mut s = com_html(r#"<div style="height:3000px"></div>"#);
        let r = respostas(&s.handle(r#"{"id":5,"method":"Page.getLayoutMetrics","params":{}}"#));
        let altura = r[0]["result"]["cssContentSize"]["height"].as_f64().unwrap();
        assert!(altura >= 3000.0, "contentSize veio {altura}");
        assert_eq!(r[0]["result"]["cssLayoutViewport"]["clientWidth"], 800.0);
    }

    #[test]
    fn viewport_emulado_muda_a_largura_de_layout() {
        let mut s = com_html("<p>oi</p>");
        s.handle(
            r#"{"id":2,"method":"Emulation.setDeviceMetricsOverride","params":{"width":1200,"height":400,"deviceScaleFactor":1}}"#,
        );
        let bytes = png_de(&mut s, json!({}));
        assert_eq!(dimensoes_png(&bytes), (1200, 400));
    }

    #[test]
    fn screenshot_sem_html_nao_panica() {
        let mut s = Session::new();
        let bytes = png_de(&mut s, json!({}));
        assert_eq!(dimensoes_png(&bytes), (800, 600));
    }
}
