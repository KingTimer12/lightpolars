# Motor de renderização CDP — fluxos de PDF — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Servir os quatro fluxos de `page.pdf()` a partir de um
servidor CDP próprio em Rust, produzindo PDF A4 com texto selecionável, sem Chromium.

**Architecture:** `blitz-dom` dá layout ao HTML numa largura fixa; extraímos uma
display list própria (caixas + runs de glifos); `paginate` corta em páginas A4 e
posiciona header/footer; `pdf-out` emite glifos como texto vetorial via `printpdf`;
`cdp-server` traduz o protocolo e emite os eventos de ciclo de vida que o Puppeteer
espera.

**Tech Stack:** Rust 2024, `blitz-dom` 0.2.4, `blitz-html` 0.2.0, `parley` 0.11,
`printpdf` 0.12.8, `tokio-tungstenite`, `serde_json`.

**Spec:** `docs/superpowers/specs/2026-09-17-motor-render-cdp-design.md`

## Global Constraints

- Rust edition 2024, toolchain 1.96+.
- **`render-core` resolve exclusivamente `data:` URI.** Qualquer outro esquema
  (`file:`, `http:`, `https:`, protocolo-relativo) resolve como recurso vazio e
  **nunca** abre socket ou arquivo. Invariante de segurança: o handler
  `page.on('request')` do Node deixa de valer quando o endpoint do CDP muda.
- Unidades internas em CSS px a 96dpi. Conversões: `mm * 96/25.4`, `cm * 96/2.54`,
  `pt * 96/72`. PDF em pt: `px * 0.75`.
- A4 retrato = 793.7 x 1122.5 px. Paisagem = 1122.5 x 793.7 px.
- `paginate` não depende de `blitz` nem de `printpdf` — só de `render-ir`.

**Desvio consciente do spec:** o spec lista 5 crates; este plano adiciona uma sexta,
`render-ir`, contendo só os tipos de dados trocados entre `render-core`, `paginate` e
`pdf-out`. Sem ela, `paginate` teria que depender de `render-core` (e portanto do
blitz) só para nomear tipos, quebrando a fronteira que o spec pede. Os 5 papéis do
spec permanecem.

## Estrutura de arquivos

```
Cargo.toml                       workspace
crates/render-ir/src/lib.rs      tipos puros: Rect, Glyph, TextRun, BoxItem, DisplayList
crates/render-core/src/lib.rs    API pública: render_html -> DisplayList
crates/render-core/src/resource.rs  resolução data:-only (invariante de segurança)
crates/render-core/src/extract.rs   documento blitz -> DisplayList
crates/paginate/src/lib.rs       PageGeometry, paginate() -> Vec<Page>
crates/pdf-out/src/lib.rs        Vec<Page> -> bytes de PDF
crates/cdp-server/src/main.rs    binário: WebSocket + loop de sessão
crates/cdp-server/src/session.rs roteamento de comandos CDP e eventos
crates/cdp-server/src/params.rs  parsing de printToPDF (margens, format, landscape)
```

---

### Task 1: Workspace e tipos da display list

**Files:**
- Modify: `Cargo.toml`
- Delete: `src/main.rs`
- Create: `crates/render-ir/Cargo.toml`
- Create/Test: `crates/render-ir/src/lib.rs`

**Interfaces:**
- Consumes: nada.
- Produces: `render_ir::{Rect, Glyph, TextRun, BoxItem, DisplayList, px_from_mm, px_from_cm, px_from_pt}`.

- [ ] **Step 1: Escrever o teste que falha**

Em `crates/render-ir/src/lib.rs`, ao final:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_bottom_e_soma_de_y_com_altura() {
        let r = Rect { x: 10.0, y: 20.0, width: 100.0, height: 30.0 };
        assert_eq!(r.bottom(), 50.0);
    }

    #[test]
    fn conversoes_de_unidade_para_px_a_96dpi() {
        assert!((px_from_mm(25.4) - 96.0).abs() < 1e-6);
        assert!((px_from_cm(2.54) - 96.0).abs() < 1e-6);
        assert!((px_from_pt(72.0) - 96.0).abs() < 1e-6);
    }

    #[test]
    fn display_list_vazia_tem_altura_zero() {
        let dl = DisplayList::default();
        assert_eq!(dl.content_height(), 0.0);
    }

    #[test]
    fn content_height_e_o_maior_bottom_entre_itens() {
        let mut dl = DisplayList::default();
        dl.boxes.push(BoxItem {
            rect: Rect { x: 0.0, y: 0.0, width: 10.0, height: 40.0 },
            background: None,
            border_color: None,
            border_width: 0.0,
        });
        dl.texts.push(TextRun {
            origin_x: 0.0,
            baseline_y: 120.0,
            font_index: 0,
            font_size_px: 12.0,
            color: [0, 0, 0],
            glyphs: vec![],
            text: String::new(),
        });
        assert_eq!(dl.content_height(), 120.0);
    }
}
```

- [ ] **Step 2: Rodar o teste e confirmar que falha**

Run: `cargo test -p render-ir`
Expected: FAIL — o pacote ainda não existe / tipos não definidos.

- [ ] **Step 3: Criar o workspace**

Substituir o `Cargo.toml` da raiz por:

```toml
[workspace]
resolver = "3"
members = ["crates/render-ir"]

[workspace.package]
edition = "2024"
version = "0.1.0"

[workspace.dependencies]
render-ir = { path = "crates/render-ir" }
```

Remover o crate antigo: `rm src/main.rs && rmdir src`

`crates/render-ir/Cargo.toml`:

```toml
[package]
name = "render-ir"
edition.workspace = true
version.workspace = true

[dependencies]
```

- [ ] **Step 4: Escrever a implementação mínima**

No topo de `crates/render-ir/src/lib.rs`, antes do módulo de testes:

```rust
//! Tipos puros trocados entre render-core, paginate e pdf-out.
//! Sem dependência de blitz ou printpdf — essa é a fronteira do desenho.

pub fn px_from_mm(mm: f32) -> f32 { mm * 96.0 / 25.4 }
pub fn px_from_cm(cm: f32) -> f32 { cm * 96.0 / 2.54 }
pub fn px_from_pt(pt: f32) -> f32 { pt * 96.0 / 72.0 }

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn bottom(&self) -> f32 { self.y + self.height }
}

/// Um glifo já posicionado pelo shaping, em px, relativo à origem do run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glyph {
    pub id: u16,
    pub x: f32,
    pub y: f32,
}

/// Sequência de glifos de uma mesma fonte e tamanho, numa mesma linha.
#[derive(Debug, Clone, PartialEq)]
pub struct TextRun {
    pub origin_x: f32,
    pub baseline_y: f32,
    /// Índice em `DisplayList::fonts`.
    pub font_index: usize,
    pub font_size_px: f32,
    pub color: [u8; 3],
    pub glyphs: Vec<Glyph>,
    /// Texto original do run, para o mapa ToUnicode do PDF.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoxItem {
    pub rect: Rect,
    pub background: Option<[u8; 3]>,
    pub border_color: Option<[u8; 3]>,
    pub border_width: f32,
}

/// Bytes de uma fonte usada pelo documento, com o índice da face no arquivo.
#[derive(Debug, Clone, PartialEq)]
pub struct FontResource {
    pub bytes: Vec<u8>,
    pub face_index: usize,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DisplayList {
    pub boxes: Vec<BoxItem>,
    pub texts: Vec<TextRun>,
    pub fonts: Vec<FontResource>,
    pub width: f32,
}

impl DisplayList {
    /// Altura ocupada pelo conteúdo. Base do corte de páginas.
    pub fn content_height(&self) -> f32 {
        let from_boxes = self.boxes.iter().map(|b| b.rect.bottom());
        let from_texts = self.texts.iter().map(|t| t.baseline_y);
        from_boxes
            .chain(from_texts)
            .fold(0.0_f32, |acc, v| acc.max(v))
    }
}
```

- [ ] **Step 5: Rodar o teste e confirmar que passa**

Run: `cargo test -p render-ir`
Expected: PASS, 4 testes.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/render-ir
git rm -r --cached src 2>/dev/null; true
git commit -m "feat(render-ir): tipos da display list e conversões de unidade"
```

---

### Task 2: Resolução de recursos restrita a `data:`

Invariante de segurança do projeto. Vem antes do layout de propósito: nenhum código
que resolve URL deve existir antes do guarda que o limita.

**Files:**
- Create: `crates/render-core/Cargo.toml`
- Create: `crates/render-core/src/lib.rs`
- Create/Test: `crates/render-core/src/resource.rs`

**Interfaces:**
- Consumes: nada.
- Produces: `render_core::resource::resolve_data_uri(&str) -> Option<Vec<u8>>`.

- [ ] **Step 1: Escrever o teste que falha**

Em `crates/render-core/src/resource.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodifica_data_uri_base64() {
        // "Oi" em base64 é "T2k="
        let out = resolve_data_uri("data:text/plain;base64,T2k=").unwrap();
        assert_eq!(out, b"Oi");
    }

    #[test]
    fn decodifica_data_uri_sem_base64() {
        let out = resolve_data_uri("data:text/plain,Oi").unwrap();
        assert_eq!(out, b"Oi");
    }

    #[test]
    fn recusa_http_https_e_file() {
        assert!(resolve_data_uri("http://exemplo.com/a.png").is_none());
        assert!(resolve_data_uri("https://exemplo.com/a.png").is_none());
        assert!(resolve_data_uri("file:///etc/passwd").is_none());
        assert!(resolve_data_uri("//exemplo.com/a.png").is_none());
        assert!(resolve_data_uri("/var/secret").is_none());
    }

    #[test]
    fn recusa_data_com_maiusculas_disfarcando_outro_esquema() {
        // Não deve ser confundido com data:
        assert!(resolve_data_uri("javascript:alert(1)").is_none());
        // data: legítimo em caixa alta continua válido
        assert!(resolve_data_uri("DATA:text/plain,Oi").is_some());
    }

    #[test]
    fn base64_invalido_retorna_none_em_vez_de_panicar() {
        assert!(resolve_data_uri("data:image/png;base64,!!!não-é-base64!!!").is_none());
    }

    #[test]
    fn entrada_multibyte_nao_panica() {
        // "é" ocupa os bytes 4 e 5: um corte em 5 cairia no meio do caractere.
        assert!(resolve_data_uri("aaaaé").is_none());
        assert!(resolve_data_uri("é").is_none());
        assert!(resolve_data_uri("dataé").is_none());
        assert_eq!(resolve_data_uri("data:text/plain,ação").unwrap(), "ação".as_bytes());
    }
}
```

- [ ] **Step 2: Rodar o teste e confirmar que falha**

Run: `cargo test -p render-core`
Expected: FAIL — pacote/função inexistente.

- [ ] **Step 3: Escrever a implementação mínima**

`crates/render-core/Cargo.toml`:

```toml
[package]
name = "render-core"
edition.workspace = true
version.workspace = true

[dependencies]
render-ir.workspace = true
base64 = "0.22"
percent-encoding = "2"
```

Adicionar `"crates/render-core"` aos `members` do workspace e
`render-core = { path = "crates/render-core" }` em `[workspace.dependencies]`.

`crates/render-core/src/lib.rs`:

```rust
pub mod resource;
```

`crates/render-core/src/resource.rs`, acima do módulo de testes:

```rust
//! Resolução de recursos referenciados pelo HTML.
//!
//! REGRA DE SEGURANÇA: só `data:` é aceito. O HTML processado por este motor é
//! não confiável (em helpers/_pdf.js ele é gerado por IA). Qualquer outro esquema
//! resolve como ausente, e nenhuma função deste módulo abre socket ou arquivo.

use base64::Engine as _;

pub fn resolve_data_uri(uri: &str) -> Option<Vec<u8>> {
    let rest = strip_data_prefix(uri)?;
    let (meta, payload) = rest.split_once(',')?;

    if meta.rsplit(';').any(|seg| seg.eq_ignore_ascii_case("base64")) {
        base64::engine::general_purpose::STANDARD
            .decode(payload.trim())
            .ok()
    } else {
        Some(
            percent_encoding::percent_decode_str(payload)
                .collect::<Vec<u8>>(),
        )
    }
}

fn strip_data_prefix(uri: &str) -> Option<&str> {
    // Comparação em bytes de propósito: `uri[..5]` fatiaria `str` por limite de
    // caractere e entraria em pânico com entrada multibyte adversária (ex.: "aaaaé",
    // onde o byte 5 cai no meio do 'é'). `bytes[..5]` nunca panica por alinhamento,
    // e `uri[5..]` é seguro porque um prefixo `data:` é ASCII puro.
    let bytes = uri.as_bytes();
    if bytes.len() >= 5 && bytes[..5].eq_ignore_ascii_case(b"data:") {
        Some(&uri[5..])
    } else {
        None
    }
}
```

- [ ] **Step 4: Rodar o teste e confirmar que passa**

Run: `cargo test -p render-core`
Expected: PASS, 6 testes.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/render-core
git commit -m "feat(render-core): resolução de recursos restrita a data: URI"
```

---

### Task 3: Layout do HTML e extração da display list

**Files:**
- Modify: `crates/render-core/Cargo.toml`
- Modify: `crates/render-core/src/lib.rs`
- Create/Test: `crates/render-core/src/extract.rs`

**Interfaces:**
- Consumes: `render_ir::{DisplayList, TextRun, Glyph, BoxItem, Rect, FontResource}`.
- Produces: `render_core::render_html(html: &str, width_px: f32) -> DisplayList`.

Notas de API confirmadas por spike, para não perder tempo:
`HtmlDocument::from_html(html, DocumentConfig::default())`;
`doc.set_viewport(Viewport::new(w_u32, h_u32, 1.0, ColorScheme::Light))`;
`doc.resolve(0.0)`; iteração por `doc.tree().iter()` (o campo `nodes` é privado);
layout em `node.final_layout`; texto em `node.element_data().inline_layout_data`,
que é um `parley::Layout` da **parley 0.6** (versão que blitz-dom 0.2.4 fixa) —
os glifos vêm de `line.items()` filtrando `PositionedLayoutItem::GlyphRun`.

- [ ] **Step 1: Escrever o teste que falha**

Em `crates/render-core/src/extract.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const HTML_PARAGRAFO: &str = r#"<!DOCTYPE html><html><head><style>
        body { margin: 0; font-family: Helvetica, Arial, sans-serif; font-size: 16px; }
        </style></head><body><p>Texto de teste</p></body></html>"#;

    #[test]
    fn extrai_glifos_de_um_paragrafo() {
        let dl = render_html(HTML_PARAGRAFO, 643.0);
        let total: usize = dl.texts.iter().map(|t| t.glyphs.len()).sum();
        assert!(total >= 13, "esperava ao menos um glifo por caractere, veio {total}");
        assert!(!dl.fonts.is_empty(), "nenhuma fonte coletada");
    }

    #[test]
    fn runs_tem_baseline_positiva_e_dentro_da_largura() {
        let dl = render_html(HTML_PARAGRAFO, 643.0);
        for run in &dl.texts {
            assert!(run.baseline_y > 0.0, "baseline não posicionada");
            assert!(run.origin_x >= 0.0 && run.origin_x < 643.0);
            assert!(run.font_index < dl.fonts.len(), "font_index fora de fonts");
        }
    }

    #[test]
    fn texto_mais_longo_ocupa_mais_altura() {
        let curto = render_html(HTML_PARAGRAFO, 300.0);
        let longo_html = HTML_PARAGRAFO.replace(
            "Texto de teste",
            &"Texto de teste bem mais longo para forçar quebra em várias linhas. ".repeat(10),
        );
        let longo = render_html(&longo_html, 300.0);
        assert!(longo.content_height() > curto.content_height());
    }

    #[test]
    fn celulas_de_tabela_viram_caixas_posicionadas() {
        let html = r#"<!DOCTYPE html><html><head><style>
            body { margin: 0; }
            table { width: 100%; font-size: 10pt; }
            td { border: 1px solid #000; padding: 4px; }
            </style></head><body><table>
            <tr><td>Alpha</td><td>Beta</td></tr>
            </table></body></html>"#;
        let dl = render_html(html, 600.0);
        let com_borda: Vec<_> = dl.boxes.iter().filter(|b| b.border_width > 0.0).collect();
        assert!(com_borda.len() >= 2, "esperava caixas de célula com borda");
        // As duas células ficam lado a lado, não empilhadas.
        let xs: Vec<f32> = com_borda.iter().map(|b| b.rect.x).collect();
        assert!(xs.iter().any(|x| *x > 0.0), "células não foram posicionadas em colunas");
    }

    #[test]
    fn largura_da_display_list_e_a_largura_pedida() {
        let dl = render_html(HTML_PARAGRAFO, 500.0);
        assert_eq!(dl.width, 500.0);
    }
}
```

- [ ] **Step 2: Rodar o teste e confirmar que falha**

Run: `cargo test -p render-core extract`
Expected: FAIL — `render_html` não existe.

- [ ] **Step 3: Escrever a implementação**

Somar a `crates/render-core/Cargo.toml`:

```toml
blitz-dom = "0.2.4"
blitz-html = "0.2.0"
blitz-traits = "0.2"
parley = "0.11"
```

`crates/render-core/src/lib.rs`:

```rust
pub mod extract;
pub mod resource;

pub use extract::render_html;
```

`crates/render-core/src/extract.rs`, acima dos testes:

```rust
//! Dá layout ao HTML com o blitz e extrai uma display list própria.
//! Esta é a única crate que conhece os tipos do blitz — a fronteira existe para
//! que trocar o miolo de layout não se propague para paginate e pdf-out.

use blitz_dom::DocumentConfig;
use blitz_html::HtmlDocument;
use blitz_traits::shell::{ColorScheme, Viewport};
use render_ir::{BoxItem, DisplayList, FontResource, Glyph, Rect, TextRun};

/// Altura do viewport usada para o layout. O documento é contínuo: a altura real
/// sai de `DisplayList::content_height`, e o corte em páginas é de `paginate`.
const ALTURA_LAYOUT_PX: u32 = 20_000;

pub fn render_html(html: &str, width_px: f32) -> DisplayList {
    let mut doc = HtmlDocument::from_html(html, DocumentConfig::default());
    doc.set_viewport(Viewport::new(
        width_px as u32,
        ALTURA_LAYOUT_PX,
        1.0,
        ColorScheme::Light,
    ));
    doc.resolve(0.0);

    let mut dl = DisplayList { width: width_px, ..Default::default() };
    let mut fontes: Vec<FontResource> = Vec::new();

    for (_id, node) in doc.tree().iter() {
        let layout = node.final_layout;
        let x = layout.location.x;
        let y = layout.location.y;

        let Some(el) = node.element_data() else { continue };

        let estilo = node.primary_styles();
        let (fundo, borda_cor, borda_largura) = match estilo.as_ref() {
            Some(s) => cores_da_caixa(s),
            None => (None, None, 0.0),
        };

        if fundo.is_some() || borda_largura > 0.0 {
            dl.boxes.push(BoxItem {
                rect: Rect { x, y, width: layout.size.width, height: layout.size.height },
                background: fundo,
                border_color: borda_cor,
                border_width: borda_largura,
            });
        }

        let Some(text_layout) = el.inline_layout_data.as_ref() else { continue };

        for line in text_layout.layout.lines() {
            let baseline = line.metrics().baseline;
            for run in line.runs() {
                let font = run.font();
                let font_index = indice_da_fonte(&mut fontes, font);
                let tamanho = run.font_size();

                let mut glifos = Vec::new();
                let mut origem_x = None;
                for cluster in run.clusters() {
                    for g in cluster.glyphs() {
                        if origem_x.is_none() {
                            origem_x = Some(g.x);
                        }
                        glifos.push(Glyph { id: g.id, x: g.x, y: g.y });
                    }
                }
                if glifos.is_empty() {
                    continue;
                }

                let inicio = run.text_range().start;
                let fim = run.text_range().end;
                let texto = text_layout
                    .text
                    .get(inicio..fim)
                    .unwrap_or_default()
                    .to_string();

                dl.texts.push(TextRun {
                    origin_x: x + origem_x.unwrap_or(0.0),
                    baseline_y: y + baseline,
                    font_index,
                    font_size_px: tamanho,
                    color: [0, 0, 0],
                    glyphs: glifos,
                    text: texto,
                });
            }
        }
    }

    dl.fonts = fontes;
    dl
}

/// Guarda a fonte uma única vez e devolve seu índice em `DisplayList::fonts`.
fn indice_da_fonte(fontes: &mut Vec<FontResource>, font: &parley::FontData) -> usize {
    let bytes = font.data.as_ref();
    let face = font.index as usize;
    if let Some(pos) = fontes
        .iter()
        .position(|f| f.face_index == face && f.bytes.len() == bytes.len() && f.bytes == bytes)
    {
        return pos;
    }
    fontes.push(FontResource { bytes: bytes.to_vec(), face_index: face });
    fontes.len() - 1
}

fn cores_da_caixa(
    estilo: &blitz_dom::node::Style,
) -> (Option<[u8; 3]>, Option<[u8; 3]>, f32) {
    let fundo = estilo.get_background().background_color.as_absolute().map(rgb);
    let borda = estilo.get_border();
    let largura = borda.border_top_width.to_f32_px();
    let cor = borda.border_top_color.as_absolute().map(rgb);
    (fundo, cor, largura)
}

fn rgb(c: &style::color::AbsoluteColor) -> [u8; 3] {
    let srgb = c.to_color_space(style::color::ColorSpace::Srgb);
    [
        (srgb.components.0 * 255.0).clamp(0.0, 255.0) as u8,
        (srgb.components.1 * 255.0).clamp(0.0, 255.0) as u8,
        (srgb.components.2 * 255.0).clamp(0.0, 255.0) as u8,
    ]
}
```

- [ ] **Step 4: Ajustar contra a API real e fazer passar**

Run: `cargo test -p render-core extract`

Os nomes de acesso a estilo computado (`primary_styles`, `get_background`,
`get_border`, `FontData::data`/`index`, `Run::font_size`, `Line::metrics().baseline`)
vêm do stylo/parley e podem divergir na 0.2.4. **Não invente nomes**: quando o
compilador reclamar, leia a fonte vendorizada e corrija.

```bash
R=$(find ~/.cargo/registry/src -maxdepth 2 -name 'blitz-dom-0.2.4')
grep -rn "pub fn primary_styles" $R/src/node/node.rs
P=$(find ~/.cargo/registry/src -maxdepth 2 -name 'parley-0.11.1')
grep -rnE "pub fn (font_size|font|metrics)\s*\(" $P/src/layout/run.rs $P/src/layout/line.rs
grep -rnE "pub (data|index)" $P/src/font.rs
```

Expected ao final: PASS, 5 testes.

- [ ] **Step 5: Commit**

```bash
git add crates/render-core Cargo.toml
git commit -m "feat(render-core): layout via blitz e extração da display list"
```

---

### Task 4: Paginação A4 com margens, paisagem e faixa de header

**Files:**
- Create: `crates/paginate/Cargo.toml`
- Create/Test: `crates/paginate/src/lib.rs`

**Interfaces:**
- Consumes: `render_ir::{DisplayList, Rect, TextRun, BoxItem}`.
- Produces: `paginate::{PageGeometry, Page, Margins, paginate}`.
  - `PageGeometry::a4(landscape: bool, margins: Margins) -> PageGeometry`
  - `PageGeometry::{sheet_width, sheet_height, content_width, content_height}`
  - `paginate(content: &DisplayList, header: Option<&DisplayList>, footer: Option<&DisplayList>, geo: &PageGeometry) -> Vec<Page>`
  - `Page { boxes: Vec<BoxItem>, texts: Vec<TextRun> }` — coordenadas já em
    espaço de folha, com as margens aplicadas.

- [x] **Step 1: Escrever o teste que falha**

`crates/paginate/src/lib.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use render_ir::{BoxItem, DisplayList, Glyph, Rect, TextRun};

    fn margens_zero() -> Margins {
        Margins { top: 0.0, right: 0.0, bottom: 0.0, left: 0.0 }
    }

    fn texto_em(y: f32) -> TextRun {
        TextRun {
            origin_x: 0.0,
            baseline_y: y,
            font_index: 0,
            font_size_px: 12.0,
            color: [0, 0, 0],
            glyphs: vec![Glyph { id: 1, x: 0.0, y: 0.0 }],
            text: "x".into(),
        }
    }

    #[test]
    fn a4_retrato_e_paisagem_trocam_os_lados() {
        let retrato = PageGeometry::a4(false, margens_zero());
        assert!((retrato.sheet_width - 793.7).abs() < 0.1);
        assert!((retrato.sheet_height - 1122.5).abs() < 0.1);

        let paisagem = PageGeometry::a4(true, margens_zero());
        assert!((paisagem.sheet_width - 1122.5).abs() < 0.1);
        assert!((paisagem.sheet_height - 793.7).abs() < 0.1);
    }

    #[test]
    fn margens_reduzem_a_area_de_conteudo() {
        let m = Margins {
            top: render_ir::px_from_mm(51.0),
            right: render_ir::px_from_mm(15.0),
            bottom: render_ir::px_from_mm(20.0),
            left: render_ir::px_from_mm(15.0),
        };
        let geo = PageGeometry::a4(false, m);
        assert!((geo.content_width() - (793.7 - m.left - m.right)).abs() < 0.1);
        assert!((geo.content_height() - (1122.5 - m.top - m.bottom)).abs() < 0.1);
    }

    #[test]
    fn conteudo_curto_gera_uma_pagina_so() {
        let mut dl = DisplayList::default();
        dl.texts.push(texto_em(100.0));
        let geo = PageGeometry::a4(false, margens_zero());
        let pages = paginate(&dl, None, None, &geo);
        assert_eq!(pages.len(), 1);
    }

    #[test]
    fn conteudo_longo_se_divide_em_paginas() {
        let mut dl = DisplayList::default();
        // Três alturas de página de conteúdo.
        let geo = PageGeometry::a4(false, margens_zero());
        let h = geo.content_height();
        for i in 0..3 {
            dl.texts.push(texto_em(i as f32 * h + 10.0));
        }
        let pages = paginate(&dl, None, None, &geo);
        assert_eq!(pages.len(), 3);
        assert_eq!(pages[0].texts.len(), 1);
        assert_eq!(pages[1].texts.len(), 1);
        assert_eq!(pages[2].texts.len(), 1);
    }

    #[test]
    fn coordenadas_sao_reescritas_para_o_espaco_da_pagina() {
        let mut dl = DisplayList::default();
        let geo = PageGeometry::a4(false, Margins {
            top: 100.0, right: 0.0, bottom: 0.0, left: 50.0,
        });
        let h = geo.content_height();
        dl.texts.push(texto_em(h + 10.0)); // primeira linha da segunda página
        let pages = paginate(&dl, None, None, &geo);
        assert_eq!(pages.len(), 2);
        let run = &pages[1].texts[0];
        // y volta para o topo da área de conteúdo da página 2, deslocado pela margem
        assert!((run.baseline_y - (100.0 + 10.0)).abs() < 0.001);
        assert!((run.origin_x - 50.0).abs() < 0.001);
    }

    #[test]
    fn header_e_repetido_em_todas_as_paginas_na_faixa_da_margem() {
        let mut dl = DisplayList::default();
        let geo = PageGeometry::a4(false, Margins {
            top: 100.0, right: 0.0, bottom: 0.0, left: 0.0,
        });
        let h = geo.content_height();
        dl.texts.push(texto_em(10.0));
        dl.texts.push(texto_em(h + 10.0));

        let mut header = DisplayList::default();
        header.texts.push(texto_em(20.0));

        let pages = paginate(&dl, Some(&header), None, &geo);
        assert_eq!(pages.len(), 2);
        for page in &pages {
            let na_faixa = page.texts.iter().filter(|t| t.baseline_y < 100.0).count();
            assert_eq!(na_faixa, 1, "header deveria aparecer uma vez por página");
        }
    }

    #[test]
    fn corte_nao_parte_uma_caixa_ao_meio() {
        let mut dl = DisplayList::default();
        let geo = PageGeometry::a4(false, margens_zero());
        let h = geo.content_height();
        // Caixa que começa antes do corte e termina depois dele.
        dl.boxes.push(BoxItem {
            rect: Rect { x: 0.0, y: h - 20.0, width: 100.0, height: 60.0 },
            background: None,
            border_color: Some([0, 0, 0]),
            border_width: 1.0,
        });
        let pages = paginate(&dl, None, None, &geo);
        // A caixa inteira desce para a página seguinte em vez de ser cortada.
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].boxes.len(), 0);
        assert_eq!(pages[1].boxes.len(), 1);
    }
}
```

- [x] **Step 2: Rodar o teste e confirmar que falha**

Run: `cargo test -p paginate`
Expected: FAIL — pacote inexistente.

- [x] **Step 3: Escrever a implementação**

`crates/paginate/Cargo.toml`:

```toml
[package]
name = "paginate"
edition.workspace = true
version.workspace = true

[dependencies]
render-ir.workspace = true
```

Adicionar aos `members` e às `[workspace.dependencies]` como nas tasks anteriores.

`crates/paginate/src/lib.rs`, acima dos testes:

```rust
//! Corte de um documento contínuo em páginas.
//! Aritmética pura sobre retângulos: não renderiza nada e não conhece blitz.

use render_ir::{BoxItem, DisplayList, TextRun};

pub const A4_LARGURA_PX: f32 = 793.7;
pub const A4_ALTURA_PX: f32 = 1122.5;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Margins {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageGeometry {
    pub sheet_width: f32,
    pub sheet_height: f32,
    pub margins: Margins,
}

impl PageGeometry {
    pub fn a4(landscape: bool, margins: Margins) -> Self {
        let (w, h) = if landscape {
            (A4_ALTURA_PX, A4_LARGURA_PX)
        } else {
            (A4_LARGURA_PX, A4_ALTURA_PX)
        };
        Self { sheet_width: w, sheet_height: h, margins }
    }

    pub fn content_width(&self) -> f32 {
        self.sheet_width - self.margins.left - self.margins.right
    }

    pub fn content_height(&self) -> f32 {
        self.sheet_height - self.margins.top - self.margins.bottom
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Page {
    pub boxes: Vec<BoxItem>,
    pub texts: Vec<TextRun>,
}

pub fn paginate(
    content: &DisplayList,
    header: Option<&DisplayList>,
    footer: Option<&DisplayList>,
    geo: &PageGeometry,
) -> Vec<Page> {
    let altura_util = geo.content_height();
    if altura_util <= 0.0 {
        return vec![Page::default()];
    }

    let cortes = calcular_cortes(content, altura_util);
    let mut paginas = Vec::with_capacity(cortes.len());

    for faixa in &cortes {
        let mut page = Page::default();

        if let Some(h) = header {
            empilhar(&mut page, h, geo.margins.left, 0.0);
        }
        if let Some(f) = footer {
            let base = geo.sheet_height - geo.margins.bottom;
            empilhar(&mut page, f, geo.margins.left, base);
        }

        let deslocamento = geo.margins.top - faixa.inicio;

        for b in &content.boxes {
            if b.rect.y >= faixa.inicio && b.rect.y < faixa.fim {
                let mut novo = b.clone();
                novo.rect.x += geo.margins.left;
                novo.rect.y += deslocamento;
                page.boxes.push(novo);
            }
        }
        for t in &content.texts {
            if t.baseline_y >= faixa.inicio && t.baseline_y < faixa.fim {
                let mut novo = t.clone();
                novo.origin_x += geo.margins.left;
                novo.baseline_y += deslocamento;
                page.texts.push(novo);
            }
        }

        paginas.push(page);
    }

    if paginas.is_empty() {
        paginas.push(Page::default());
    }
    paginas
}

struct Faixa {
    inicio: f32,
    fim: f32,
}

/// Calcula os limites de cada página, recuando o corte para não partir uma caixa.
/// É um `break-inside: avoid` pobre — suficiente para fluxo linear de editor rico.
fn calcular_cortes(content: &DisplayList, altura_util: f32) -> Vec<Faixa> {
    let total = content.content_height();
    let mut faixas = Vec::new();
    let mut inicio = 0.0_f32;

    while inicio < total || faixas.is_empty() {
        let limite_natural = inicio + altura_util;
        let fim = recuar_para_limite_de_caixa(content, inicio, limite_natural);
        faixas.push(Faixa { inicio, fim });
        if fim <= inicio {
            break; // proteção contra laço infinito
        }
        inicio = fim;
        if faixas.len() > 10_000 {
            break;
        }
    }
    faixas
}

fn recuar_para_limite_de_caixa(content: &DisplayList, inicio: f32, limite: f32) -> f32 {
    let mut corte = limite;
    for b in &content.boxes {
        let topo = b.rect.y;
        let base = b.rect.bottom();
        // Caixa atravessa o corte: empurra a caixa inteira para a página seguinte.
        if topo > inicio && topo < corte && base > corte {
            corte = corte.min(topo);
        }
    }
    corte
}

fn empilhar(page: &mut Page, fonte: &DisplayList, dx: f32, dy: f32) {
    for b in &fonte.boxes {
        let mut novo = b.clone();
        novo.rect.x += dx;
        novo.rect.y += dy;
        page.boxes.push(novo);
    }
    for t in &fonte.texts {
        let mut novo = t.clone();
        novo.origin_x += dx;
        novo.baseline_y += dy;
        page.texts.push(novo);
    }
}
```

- [x] **Step 4: Rodar os testes e confirmar que passam**

Run: `cargo test -p paginate`
Expected: PASS, 7 testes.

- [x] **Step 5: Commit**

```bash
git add crates/paginate Cargo.toml
git commit -m "feat(paginate): corte A4 com margens, paisagem e faixa de header"
```

---

### Task 5: Emissão de PDF com texto selecionável

**Files:**
- Create: `crates/pdf-out/Cargo.toml`
- Create/Test: `crates/pdf-out/src/lib.rs`

**Interfaces:**
- Consumes: `paginate::{Page, PageGeometry}`, `render_ir::FontResource`.
- Produces: `pdf_out::render_pdf(pages: &[Page], fonts: &[FontResource], geo: &PageGeometry) -> Vec<u8>`.

API do `printpdf` 0.12.8 já verificada: `ParsedFont::from_bytes(&bytes, index, &mut warnings)`,
`doc.add_font(&parsed) -> FontId`, `Op::SetTextMatrix { matrix }`,
`TextItem::GlyphIds(Vec<u16>)`, `doc.with_pages(...)`, `doc.save(&opts, &mut warnings)`.
Subsetting e ToUnicode são tratados pela própria crate.

- [x] **Step 1: Escrever o teste que falha**

`crates/pdf-out/src/lib.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use paginate::{Margins, Page, PageGeometry};
    use render_ir::{Glyph, TextRun};

    fn geo() -> PageGeometry {
        PageGeometry::a4(false, Margins::default())
    }

    #[test]
    fn pdf_vazio_ainda_e_um_pdf_valido() {
        let bytes = render_pdf(&[Page::default()], &[], &geo());
        assert!(bytes.starts_with(b"%PDF-"), "cabeçalho de PDF ausente");
        assert!(bytes.windows(5).any(|w| w == b"%%EOF"), "trailer ausente");
    }

    #[test]
    fn numero_de_paginas_no_pdf_bate_com_a_entrada() {
        let bytes = render_pdf(&[Page::default(), Page::default(), Page::default()], &[], &geo());
        let texto = String::from_utf8_lossy(&bytes);
        let ocorrencias = texto.matches("/Type /Page").count();
        assert!(ocorrencias >= 3, "esperava ao menos 3 páginas, veio {ocorrencias}");
    }

    #[test]
    fn texto_e_emitido_como_operador_de_texto_nao_como_imagem() {
        let page = Page {
            boxes: vec![],
            texts: vec![TextRun {
                origin_x: 100.0,
                baseline_y: 200.0,
                font_index: 0,
                font_size_px: 16.0,
                color: [0, 0, 0],
                glyphs: vec![Glyph { id: 36, x: 0.0, y: 0.0 }],
                text: "A".into(),
            }],
        };
        let fonte = render_ir::FontResource {
            bytes: carregar_fonte_de_teste(),
            face_index: 0,
        };
        let bytes = render_pdf(&[page], &[fonte], &geo());
        let texto = String::from_utf8_lossy(&bytes);
        assert!(texto.contains("/FontFile2") || texto.contains("/FontFile3"),
            "fonte não foi embutida");
        assert!(!texto.contains("/Subtype /Image"), "texto virou bitmap");
    }

    /// Usa uma fonte do próprio sistema para o teste não depender de fixture binária.
    fn carregar_fonte_de_teste() -> Vec<u8> {
        let candidatos = [
            "/System/Library/Fonts/Helvetica.ttc",
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        ];
        for c in candidatos {
            if let Ok(b) = std::fs::read(c) {
                return b;
            }
        }
        panic!("nenhuma fonte de teste encontrada nos caminhos conhecidos");
    }
}
```

- [x] **Step 2: Rodar o teste e confirmar que falha**

Run: `cargo test -p pdf-out`
Expected: FAIL — pacote inexistente.

- [x] **Step 3: Escrever a implementação**

`crates/pdf-out/Cargo.toml`:

```toml
[package]
name = "pdf-out"
edition.workspace = true
version.workspace = true

[dependencies]
render-ir.workspace = true
paginate.workspace = true
printpdf = "0.12.8"
```

`crates/pdf-out/src/lib.rs`, acima dos testes:

```rust
//! Emissão de PDF a partir de páginas já paginadas.
//! Texto sai como operador de texto com glifos posicionados — nunca rasterizado.

use paginate::{Page, PageGeometry};
use printpdf::*;
use render_ir::FontResource;

/// PDF trabalha em pt; o resto do motor em px a 96dpi.
fn pt(px: f32) -> f32 { px * 0.75 }

pub fn render_pdf(pages: &[Page], fonts: &[FontResource], geo: &PageGeometry) -> Vec<u8> {
    let mut doc = PdfDocument::new("documento");
    let mut avisos_fonte = Vec::new();

    let ids: Vec<Option<FontId>> = fonts
        .iter()
        .map(|f| {
            ParsedFont::from_bytes(&f.bytes, f.face_index, &mut avisos_fonte)
                .map(|parsed| doc.add_font(&parsed))
        })
        .collect();

    let largura_pt = pt(geo.sheet_width);
    let altura_pt = pt(geo.sheet_height);

    let paginas: Vec<PdfPage> = pages
        .iter()
        .map(|page| {
            let mut ops = Vec::new();

            for b in &page.boxes {
                if let Some(cor) = b.background {
                    ops.push(Op::SetFillColor { col: cor_rgb(cor) });
                    ops.push(retangulo(b.rect, altura_pt, PaintMode::Fill));
                }
                if b.border_width > 0.0 {
                    if let Some(cor) = b.border_color {
                        ops.push(Op::SetOutlineColor { col: cor_rgb(cor) });
                        ops.push(Op::SetOutlineThickness { pt: Pt(pt(b.border_width)) });
                        ops.push(retangulo(b.rect, altura_pt, PaintMode::Stroke));
                    }
                }
            }

            for run in &page.texts {
                let Some(Some(font_id)) = ids.get(run.font_index) else { continue };
                if run.glyphs.is_empty() {
                    continue;
                }

                ops.push(Op::StartTextSection);
                ops.push(Op::SetFillColor { col: cor_rgb(run.color) });
                ops.push(Op::SetFontSize {
                    size: Pt(pt(run.font_size_px)),
                    font: font_id.clone(),
                });

                // PDF tem origem embaixo à esquerda; a display list, em cima à esquerda.
                let x = pt(run.origin_x);
                let y = altura_pt - pt(run.baseline_y);
                ops.push(Op::SetTextMatrix {
                    matrix: TextMatrix::Translate(Pt(x), Pt(y)),
                });

                ops.push(Op::WriteCodepoints {
                    font: font_id.clone(),
                    cp: run
                        .glyphs
                        .iter()
                        .zip(run.text.chars().chain(std::iter::repeat('\u{0}')))
                        .map(|(g, ch)| (g.id, ch))
                        .collect(),
                });

                ops.push(Op::EndTextSection);
            }

            PdfPage::new(Mm(largura_pt / 2.834_646), Mm(altura_pt / 2.834_646), ops)
        })
        .collect();

    doc.with_pages(paginas);
    let mut avisos = Vec::new();
    doc.save(&PdfSaveOptions::default(), &mut avisos)
}

fn cor_rgb(c: [u8; 3]) -> Color {
    Color::Rgb(Rgb {
        r: c[0] as f32 / 255.0,
        g: c[1] as f32 / 255.0,
        b: c[2] as f32 / 255.0,
        icc_profile: None,
    })
}

fn retangulo(r: render_ir::Rect, altura_pt: f32, modo: PaintMode) -> Op {
    let x = pt(r.x);
    let y = altura_pt - pt(r.y) - pt(r.height);
    Op::DrawPolygon {
        polygon: Polygon {
            rings: vec![PolygonRing {
                points: vec![
                    ponto(x, y),
                    ponto(x + pt(r.width), y),
                    ponto(x + pt(r.width), y + pt(r.height)),
                    ponto(x, y + pt(r.height)),
                ],
            }],
            mode: modo,
            winding_order: WindingOrder::NonZero,
        },
    }
}

fn ponto(x: f32, y: f32) -> LinePoint {
    LinePoint {
        p: Point { x: Pt(x), y: Pt(y) },
        bezier: false,
    }
}
```

- [x] **Step 4: Ajustar contra a API real e fazer passar**

Run: `cargo test -p pdf-out`

`Op::WriteCodepoints`, `TextMatrix::Translate`, `PdfPage::new` e os campos de
`Polygon`/`Rgb` podem divergir. Conferir na fonte antes de adivinhar:

```bash
R=$(find ~/.cargo/registry/src -maxdepth 2 -name 'printpdf-0.12.8')
grep -nE "WriteCodepoints|WriteText|ShowText|SetFontSize|SetOutline" $R/src/ops.rs | head -20
grep -nE "pub enum TextMatrix" -A 10 $R/src/matrix.rs
grep -nE "pub fn new" $R/src/ops.rs | head
grep -nE "pub struct (Polygon|PolygonRing|LinePoint|Rgb)" -A 8 $R/src/graphics.rs | head -40
```

Se `WriteCodepoints` não existir nessa versão, usar
`Op::ShowText { items: vec![TextItem::GlyphIds(ids)] }` — confirmado presente em
`ops.rs:248` — e manter `run.text` para o ToUnicode via o caminho que a crate expuser.

Expected ao final: PASS, 3 testes.

- [x] **Step 5: Commit**

```bash
git add crates/pdf-out Cargo.toml
git commit -m "feat(pdf-out): emissão de PDF com glifos posicionados e fonte embutida"
```

---

### Task 6: Parsing dos parâmetros de `Page.printToPDF`

**Files:**
- Create: `crates/cdp-server/Cargo.toml`
- Create: `crates/cdp-server/src/lib.rs`
- Create/Test: `crates/cdp-server/src/params.rs`

**Interfaces:**
- Consumes: `paginate::{Margins, PageGeometry}`.
- Produces: `cdp_server::params::{geometry_from_print_params, comprimento_para_px}`.
  - `geometry_from_print_params(params: &serde_json::Value) -> PageGeometry`

O Puppeteer envia margens em **polegadas** no CDP (ele converte `'51mm'` antes de
mandar) e `paperWidth`/`paperHeight` também em polegadas. `landscape` é booleano.
Ausência de campo significa o padrão do Chrome: margem 1cm (0.393701in), papel
carta 8.5x11in — mas todos os call sites passam `format: 'A4'`, que o Puppeteer
traduz para 8.27x11.7in.

- [ ] **Step 1: Escrever o teste que falha**

`crates/cdp-server/src/params.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn polegadas_viram_px_a_96dpi() {
        assert!((comprimento_para_px(1.0) - 96.0).abs() < 1e-6);
        assert!((comprimento_para_px(0.5) - 48.0).abs() < 1e-6);
    }

    #[test]
    fn a4_retrato_a_partir_dos_parametros_do_puppeteer() {
        let p = json!({
            "paperWidth": 8.27, "paperHeight": 11.7,
            "marginTop": 0.0, "marginRight": 0.0,
            "marginBottom": 0.0, "marginLeft": 0.0,
            "landscape": false, "printBackground": true
        });
        let geo = geometry_from_print_params(&p);
        assert!((geo.sheet_width - 793.9).abs() < 1.0);
        assert!((geo.sheet_height - 1123.2).abs() < 1.0);
    }

    #[test]
    fn landscape_troca_os_lados() {
        let p = json!({
            "paperWidth": 8.27, "paperHeight": 11.7, "landscape": true
        });
        let geo = geometry_from_print_params(&p);
        assert!(geo.sheet_width > geo.sheet_height, "paisagem não aplicada");
    }

    #[test]
    fn margens_assimetricas_sao_preservadas() {
        // 51mm = 2.0079in ; 15mm = 0.5906in ; 20mm = 0.7874in
        let p = json!({
            "paperWidth": 8.27, "paperHeight": 11.7,
            "marginTop": 2.0079, "marginRight": 0.5906,
            "marginBottom": 0.7874, "marginLeft": 0.5906
        });
        let geo = geometry_from_print_params(&p);
        assert!((geo.margins.top - render_ir::px_from_mm(51.0)).abs() < 1.0);
        assert!((geo.margins.left - render_ir::px_from_mm(15.0)).abs() < 1.0);
        assert!((geo.margins.bottom - render_ir::px_from_mm(20.0)).abs() < 1.0);
    }

    #[test]
    fn parametros_ausentes_caem_no_padrao_sem_panicar() {
        let geo = geometry_from_print_params(&json!({}));
        assert!(geo.sheet_width > 0.0 && geo.sheet_height > 0.0);
        assert!(geo.content_height() > 0.0);
    }
}
```

- [ ] **Step 2: Rodar o teste e confirmar que falha**

Run: `cargo test -p cdp-server params`
Expected: FAIL — pacote inexistente.

- [ ] **Step 3: Escrever a implementação**

`crates/cdp-server/Cargo.toml`:

```toml
[package]
name = "cdp-server"
edition.workspace = true
version.workspace = true

[dependencies]
render-ir.workspace = true
render-core.workspace = true
paginate.workspace = true
pdf-out.workspace = true
serde_json = "1"
serde = { version = "1", features = ["derive"] }
tokio = { version = "1", features = ["rt-multi-thread", "macros", "net"] }
tokio-tungstenite = "0.24"
futures-util = "0.3"
```

`crates/cdp-server/src/lib.rs`:

```rust
pub mod params;
```

`crates/cdp-server/src/params.rs`, acima dos testes:

```rust
//! Tradução dos parâmetros de Page.printToPDF para a geometria de página.
//! O CDP fala em polegadas; o motor, em px a 96dpi.

use paginate::{Margins, PageGeometry};
use serde_json::Value;

const A4_LARGURA_IN: f64 = 8.27;
const A4_ALTURA_IN: f64 = 11.7;
/// Padrão do Chrome quando a margem não é informada: 1cm.
const MARGEM_PADRAO_IN: f64 = 0.393_701;

pub fn comprimento_para_px(polegadas: f64) -> f32 {
    (polegadas * 96.0) as f32
}

pub fn geometry_from_print_params(params: &Value) -> PageGeometry {
    let num = |chave: &str, padrao: f64| -> f64 {
        params.get(chave).and_then(Value::as_f64).unwrap_or(padrao)
    };

    let largura_in = num("paperWidth", A4_LARGURA_IN);
    let altura_in = num("paperHeight", A4_ALTURA_IN);
    let landscape = params
        .get("landscape")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let margins = Margins {
        top: comprimento_para_px(num("marginTop", MARGEM_PADRAO_IN)),
        right: comprimento_para_px(num("marginRight", MARGEM_PADRAO_IN)),
        bottom: comprimento_para_px(num("marginBottom", MARGEM_PADRAO_IN)),
        left: comprimento_para_px(num("marginLeft", MARGEM_PADRAO_IN)),
    };

    let (w, h) = if landscape {
        (altura_in, largura_in)
    } else {
        (largura_in, altura_in)
    };

    PageGeometry {
        sheet_width: comprimento_para_px(w),
        sheet_height: comprimento_para_px(h),
        margins,
    }
}
```

- [ ] **Step 4: Rodar os testes e confirmar que passam**

Run: `cargo test -p cdp-server params`
Expected: PASS, 5 testes.

- [ ] **Step 5: Commit**

```bash
git add crates/cdp-server Cargo.toml
git commit -m "feat(cdp-server): parsing dos parâmetros de printToPDF"
```

---

### Task 7: Sessão CDP — roteamento de comandos e eventos de ciclo de vida

**Files:**
- Modify: `crates/cdp-server/src/lib.rs`
- Create/Test: `crates/cdp-server/src/session.rs`

**Interfaces:**
- Consumes: `params::geometry_from_print_params`, `render_core::render_html`,
  `paginate::paginate`, `pdf_out::render_pdf`.
- Produces: `cdp_server::session::{Session, Saida}`.
  - `Session::new() -> Session`
  - `Session::handle(&mut self, msg: &str) -> Vec<Saida>`
  - `Saida::{Resposta(String), Evento(String)}`

Separar a máquina de estados do transporte é o que torna isto testável sem abrir
socket. O `main` só liga WebSocket a `Session::handle`.

- [ ] **Step 1: Escrever o teste que falha**

`crates/cdp-server/src/session.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn json_de(s: &str) -> Value {
        serde_json::from_str(s).expect("saída não é JSON válido")
    }

    fn respostas(saidas: &[Saida]) -> Vec<Value> {
        saidas.iter().filter_map(|s| match s {
            Saida::Resposta(t) => Some(json_de(t)),
            _ => None,
        }).collect()
    }

    fn eventos(saidas: &[Saida]) -> Vec<Value> {
        saidas.iter().filter_map(|s| match s {
            Saida::Evento(t) => Some(json_de(t)),
            _ => None,
        }).collect()
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
```

- [ ] **Step 2: Rodar o teste e confirmar que falha**

Run: `cargo test -p cdp-server session`
Expected: FAIL — `Session` não existe.

- [ ] **Step 3: Escrever a implementação**

Somar a `crates/cdp-server/Cargo.toml`: `base64 = "0.22"`.

`crates/cdp-server/src/lib.rs`:

```rust
pub mod params;
pub mod session;
```

`crates/cdp-server/src/session.rs`, acima dos testes:

```rust
//! Máquina de estados do CDP, independente de transporte.
//! O Puppeteer bloqueia num LifecycleWatcher depois de setDocumentContent:
//! sem os eventos de ciclo de vida, setContent trava até o timeout.

use base64::Engine as _;
use paginate::paginate;
use serde_json::{json, Value};

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
```

- [ ] **Step 4: Rodar os testes e confirmar que passam**

Run: `cargo test -p cdp-server`
Expected: PASS, 10 testes (5 de `params`, 5 de `session`).

- [ ] **Step 5: Commit**

```bash
git add crates/cdp-server Cargo.toml
git commit -m "feat(cdp-server): sessão CDP com eventos de ciclo de vida e printToPDF"
```

---

### Task 8: Binário WebSocket e aceitação com Puppeteer real

**Files:**
- Create: `crates/cdp-server/src/main.rs`
- Modify: `crates/cdp-server/Cargo.toml`
- Create: `tests/aceitacao/gerar_goldens.js`
- Create: `tests/aceitacao/rodar.js`
- Create: `tests/aceitacao/.gitignore`
- Create: `tests/aceitacao/README.md`

**Fixtures não são versionadas.** Os HTMLs reais e os PDFs golden ficam fora do
versionamento: carregam marca, logo em base64 e texto identificável do sistema
consumidor. O repositório guarda apenas os scripts e `casos.json` (opções de
`page.pdf`, sem conteúdo). Cada máquina gera os seus.

**Interfaces:**
- Consumes: `cdp_server::session::{Session, Saida}`.
- Produces: binário que escuta em `ws://127.0.0.1:9222`.

- [ ] **Step 1: Escrever o teste que falha**

`tests/aceitacao/rodar.js` — teste de ponta a ponta contra o motor:

```js
// Roda os fluxos reais contra o motor e compara com os goldens do Chromium.
// Uso: node tests/aceitacao/rodar.js
import puppeteer from 'puppeteer-core'
import fs from 'node:fs'
import path from 'node:path'

const CASOS = JSON.parse(fs.readFileSync('tests/aceitacao/casos.json', 'utf8'))
const DIR_GOLDEN = 'tests/aceitacao/golden'

const browser = await puppeteer.connect({
  browserWSEndpoint: 'ws://127.0.0.1:9222',
})

let falhas = 0
for (const caso of CASOS) {
  const page = await browser.newPage()
  const html = fs.readFileSync(path.join('tests/aceitacao/html', caso.html), 'utf8')
  await page.setContent(html, { waitUntil: caso.waitUntil ?? 'load' })
  const pdf = Buffer.from(await page.pdf(caso.pdfOptions))
  await page.close()

  if (!pdf.subarray(0, 5).equals(Buffer.from('%PDF-'))) {
    console.error(`FALHA ${caso.nome}: saída não é PDF`)
    falhas++
    continue
  }

  const golden = path.join(DIR_GOLDEN, `${caso.nome}.pdf`)
  if (!fs.existsSync(golden)) {
    console.error(`FALHA ${caso.nome}: golden ausente — rode gerar_goldens.js primeiro`)
    falhas++
    continue
  }

  // Comparação de estrutura, não de bytes: contagem de páginas e presença de texto.
  const paginasMotor = (pdf.toString('latin1').match(/\/Type\s*\/Page[^s]/g) || []).length
  const bytesGolden = fs.readFileSync(golden)
  const paginasGolden = (bytesGolden.toString('latin1').match(/\/Type\s*\/Page[^s]/g) || []).length
  if (paginasMotor !== paginasGolden) {
    console.error(`FALHA ${caso.nome}: ${paginasMotor} páginas, golden tem ${paginasGolden}`)
    falhas++
    continue
  }

  console.log(`OK ${caso.nome} (${paginasMotor} páginas)`)
}

await browser.disconnect()
process.exit(falhas === 0 ? 0 : 1)
```

`tests/aceitacao/gerar_goldens.js` — roda uma vez contra o Chromium atual:

```js
// Gera os PDFs de referência com o Chromium, para congelar como golden.
// Uso: node tests/aceitacao/gerar_goldens.js
import puppeteer from 'puppeteer'
import fs from 'node:fs'
import path from 'node:path'

const CASOS = JSON.parse(fs.readFileSync('tests/aceitacao/casos.json', 'utf8'))
fs.mkdirSync('tests/aceitacao/golden', { recursive: true })

const browser = await puppeteer.launch({
  headless: true,
  args: ['--no-sandbox', '--disable-setuid-sandbox', '--disable-gpu'],
})

for (const caso of CASOS) {
  const page = await browser.newPage()
  const html = fs.readFileSync(path.join('tests/aceitacao/html', caso.html), 'utf8')
  await page.setContent(html, { waitUntil: caso.waitUntil ?? 'load' })
  const pdf = await page.pdf(caso.pdfOptions)
  fs.writeFileSync(path.join('tests/aceitacao/golden', `${caso.nome}.pdf`), pdf)
  await page.close()
  console.log(`golden gerado: ${caso.nome}`)
}

await browser.close()
```

- [ ] **Step 2: Montar os casos e confirmar que o teste falha**

Criar `tests/aceitacao/casos.json` com os quatro fluxos reais, copiando as opções
verbatim dos call sites da API consumidora:

```json
[
  {
    "nome": "pdf-ia-com-header",
    "html": "pdf_ia.html",
    "waitUntil": "load",
    "pdfOptions": {
      "format": "A4",
      "printBackground": true,
      "margin": { "top": "51mm", "right": "15mm", "bottom": "20mm", "left": "15mm" }
    }
  },
  {
    "nome": "funcionarios-landscape",
    "html": "funcionarios.html",
    "waitUntil": "networkidle0",
    "pdfOptions": {
      "format": "A4",
      "landscape": true,
      "printBackground": true,
      "margin": { "top": "20mm", "right": "15mm", "bottom": "20mm", "left": "15mm" }
    }
  },
  {
    "nome": "documento-ged",
    "html": "documento.html",
    "pdfOptions": {
      "format": "A4",
      "printBackground": true,
      "margin": { "top": "0cm", "right": "2cm", "bottom": "0cm", "left": "2cm" }
    }
  },
  {
    "nome": "recibo",
    "html": "recibo.html",
    "pdfOptions": {
      "format": "A4",
      "printBackground": true,
      "margin": { "top": "0.1cm", "right": "1cm", "bottom": "1cm", "left": "1cm" }
    }
  }
]
```

Criar `tests/aceitacao/.gitignore` para manter fixtures e goldens fora do repositório:

```gitignore
html/
golden/
```

E `tests/aceitacao/README.md`:

```markdown
# Aceitação

Os HTMLs de entrada (`html/`) e os PDFs de referência (`golden/`) não são
versionados: contêm conteúdo identificável do sistema consumidor.

Para preparar a suíte numa máquina nova:

1. Copie os quatro HTMLs reais dos fluxos de `page.pdf` para `html/`, com os nomes
   declarados em `casos.json` (`pdf_ia.html`, `funcionarios.html`,
   `documento.html`, `recibo.html`). Substitua apenas as interpolações de template
   por conteúdo representativo — mantenha o CSS intacto, é ele que está sob teste.
2. Rode `node gerar_goldens.js` a partir da raiz do repositório para produzir
   `golden/` com o Chromium atual.
3. Rode `node rodar.js` com o motor no ar.
```

Depois disso, montar `html/` seguindo o README — copiando os quatro HTMLs reais dos
fluxos de `page.pdf` e trocando as interpolações por conteúdo representativo de
editor rico (parágrafos, títulos, uma tabela simples, uma imagem `data:`).

Run: `node tests/aceitacao/rodar.js`
Expected: FAIL — conexão recusada, o servidor ainda não existe.

- [ ] **Step 3: Escrever o binário**

`crates/cdp-server/src/main.rs`:

```rust
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
            envio.send(Message::Text(payload)).await?;
        }
    }
    Ok(())
}
```

Somar ao `Cargo.toml` do `cdp-server`:

```toml
[[bin]]
name = "cdp-server"
path = "src/main.rs"
```

- [ ] **Step 4: Rodar a aceitação**

```bash
node tests/aceitacao/gerar_goldens.js     # uma vez, com o Chromium atual
cargo run -p cdp-server &                 # sobe o motor
node tests/aceitacao/rodar.js
```

Expected: os quatro casos imprimem `OK` com contagem de páginas igual à do golden.

**Ressalva conhecida deste plano:** a Task 7 passa `None` para header e footer, então
o caso `pdf-ia-com-header` valida margens e paginação, **não** a logo repetida por
página. Se a contagem de páginas divergir do golden justamente nesse caso, a causa
provável é essa — e a correção é o primeiro item do próximo plano, não um ajuste
aqui. Os outros três casos não usam `displayHeaderFooter` e valem integralmente.

Se o `newPage()` travar, o handshake está incompleto: registrar a sequência real de
mensagens antes de adivinhar —

```bash
PUPPETEER_EXTRA_LAUNCH_ARGS= DEBUG="puppeteer:protocol:*" node tests/aceitacao/rodar.js 2>&1 | head -60
```

e implementar em `session.rs` o que aparecer faltando.

- [ ] **Step 5: Commit**

```bash
git add crates/cdp-server tests/aceitacao
git status --short tests/aceitacao   # confirme: nenhum arquivo de html/ ou golden/
git commit -m "feat(cdp-server): binário WebSocket e aceitação com Puppeteer real"
```

---

## Depois deste plano

- Fluxo de PNG (`captureScreenshot`) — plano próprio, depende de coletar o HTML de
  `controllers/cerimonial/_agendamentos.js` e de verificar flex/grid/SVG/WOFF2
  contra o blitz, o que ainda não foi feito.
- `headerTemplate`/`footerTemplate`: a Task 7 passa `None` para header e footer. O
  `paginate` já os suporta (Task 4) e o `_pdf.js` depende deles — ligar os dois é o
  primeiro item do próximo plano, dando layout ao template como documento separado
  na largura da folha e altura da margem.
- `border-collapse` e `rowspan`, se o conteúdo real exigir.
