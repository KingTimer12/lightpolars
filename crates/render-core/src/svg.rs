//! Suporte a SVG: rasterização de árvores usvg e reescrita de `<svg>` inline.
//!
//! O blitz-dom 0.2.4 só transforma SVG em `ImageData::Svg` quando ele chega como
//! *recurso* (`<img src=...>` ou `background-image`). Um `<svg>` escrito direto
//! no HTML vira um elemento substituído de tamanho zero e sem dado nenhum. Por
//! isso `reescrever_svg_inline` converte cada `<svg>…</svg>` do fonte em um
//! `<img src="data:image/svg+xml;base64,…">` antes do parse: assim o caminho de
//! imagem já existente cuida de parse, tamanho intrínseco e layout.
//!
//! REGRA DE SEGURANÇA: a reescrita só produz `data:`, e o conteúdo vem do
//! próprio HTML — nenhum socket ou arquivo é aberto aqui.

use base64::Engine as _;

/// Quantas amostras por px CSS ao rasterizar. SVG é vetorial e o PDF sai em
/// 72dpi (1px = 1pt); rasterizar 1:1 deixaria a logo serrilhada na impressão.
const SUPERAMOSTRAGEM: f32 = 3.0;

/// Teto por lado do bitmap gerado. Um `<svg>` ocupando uma página A4 inteira em
/// 3x dá ~1800x2500; o limite existe para um SVG absurdamente grande não virar
/// centenas de MB de RGBA.
const LADO_MAXIMO_PX: u32 = 4096;

/// Rasteriza a árvore no tamanho de caixa dado (em px CSS), devolvendo
/// `(largura, altura, RGBA8 não pré-multiplicado)`.
///
/// O `tiny-skia` trabalha com RGBA pré-multiplicado; o `printpdf` espera os
/// componentes retos (ele fatia o alfa num `/SMask`). Sem o `demultiply` as
/// bordas antialiased do SVG sairiam escurecidas.
pub fn rasterizar(tree: &usvg::Tree, largura_css: f32, altura_css: f32) -> Option<(u32, u32, Vec<u8>)> {
    if !largura_css.is_finite() || !altura_css.is_finite() || largura_css <= 0.0 || altura_css <= 0.0
    {
        return None;
    }

    let escala = escala_cabendo_no_teto(largura_css, altura_css);
    let largura = (largura_css * escala).round().max(1.0) as u32;
    let altura = (altura_css * escala).round().max(1.0) as u32;

    let mut pixmap = resvg::tiny_skia::Pixmap::new(largura, altura)?;

    // A árvore tem tamanho próprio (o viewBox); o transform leva esse tamanho
    // até o bitmap, que é a caixa do layout vezes a superamostragem.
    let tamanho = tree.size();
    if tamanho.width() <= 0.0 || tamanho.height() <= 0.0 {
        return None;
    }
    let transform = resvg::tiny_skia::Transform::from_scale(
        largura as f32 / tamanho.width(),
        altura as f32 / tamanho.height(),
    );
    resvg::render(tree, transform, &mut pixmap.as_mut());

    let mut rgba = Vec::with_capacity(largura as usize * altura as usize * 4);
    for px in pixmap.pixels() {
        let c = px.demultiply();
        rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }

    Some((largura, altura, rgba))
}

/// Superamostragem pedida, reduzida se estourar `LADO_MAXIMO_PX` em algum lado.
fn escala_cabendo_no_teto(largura_css: f32, altura_css: f32) -> f32 {
    let maior = largura_css.max(altura_css);
    SUPERAMOSTRAGEM.min(LADO_MAXIMO_PX as f32 / maior)
}

/// Atributos do `<svg>` que fazem sentido no `<img>` que o substitui. Os outros
/// (`viewBox`, `xmlns`, `fill`…) pertencem ao SVG e seguem dentro do data: URI.
const ATRIBUTOS_HERDADOS: [&str; 5] = ["id", "class", "style", "width", "height"];

/// Troca todo `<svg>…</svg>` do HTML por um `<img src="data:image/svg+xml;…">`.
pub fn reescrever_svg_inline(html: &str) -> String {
    let mut saida = String::with_capacity(html.len());
    let mut resto = html;

    while let Some(inicio) = achar_abertura_svg(resto) {
        saida.push_str(&resto[..inicio]);
        let Some(fim) = achar_fechamento_svg(&resto[inicio..]) else {
            // `<svg` sem fechamento: copia o resto cru em vez de perder conteúdo.
            resto = &resto[inicio..];
            break;
        };
        let bloco = &resto[inicio..inicio + fim];
        saida.push_str(&bloco_para_img(bloco));
        resto = &resto[inicio + fim..];
    }

    saida.push_str(resto);
    saida
}

/// Índice do próximo `<svg` que abre um elemento de verdade (e não um prefixo
/// como `<svgfoo`).
fn achar_abertura_svg(texto: &str) -> Option<usize> {
    let minusculo = texto.to_ascii_lowercase();
    let bytes = minusculo.as_bytes();
    let mut de = 0;
    while let Some(pos) = minusculo[de..].find("<svg") {
        let abs = de + pos;
        if abre_svg_em(bytes, abs) {
            return Some(abs);
        }
        de = abs + 4;
    }
    None
}

/// `true` se em `i` (num texto já minúsculo) começa a tag de abertura `<svg`, e
/// não um nome maior como `<svgfoo`.
fn abre_svg_em(bytes: &[u8], i: usize) -> bool {
    if !bytes[i..].starts_with(b"<svg") {
        return false;
    }
    matches!(
        bytes.get(i + 4),
        Some(b'>') | Some(b'/') | Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r')
    )
}

/// Comprimento do bloco `<svg>…</svg>` a partir do começo de `texto`, contando
/// aninhamento (SVG permite `<svg>` dentro de `<svg>`).
fn achar_fechamento_svg(texto: &str) -> Option<usize> {
    let minusculo = texto.to_ascii_lowercase();
    let bytes = minusculo.as_bytes();
    let mut profundidade = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        if minusculo[i..].starts_with("</svg") {
            let fim = i + minusculo[i..].find('>')? + 1;
            profundidade = profundidade.saturating_sub(1);
            if profundidade == 0 {
                return Some(fim);
            }
            i = fim;
        } else if abre_svg_em(bytes, i) {
            let fim = i + minusculo[i..].find('>')? + 1;
            // Tag vazia (`<svg ... />`) não abre nível.
            if !minusculo[i..fim].ends_with("/>") {
                profundidade += 1;
            } else if profundidade == 0 {
                return Some(fim);
            }
            i = fim;
        } else {
            i += 1;
        }
    }
    None
}

fn bloco_para_img(bloco: &str) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(bloco.as_bytes());
    let mut img = String::from("<img");
    for (nome, valor) in atributos_da_tag(bloco) {
        if ATRIBUTOS_HERDADOS.contains(&nome.to_ascii_lowercase().as_str()) {
            img.push_str(&format!(" {nome}=\"{}\"", escapar_aspas(&valor)));
        }
    }
    img.push_str(&format!(" src=\"data:image/svg+xml;base64,{b64}\" />"));
    img
}

/// Pares `nome="valor"` da tag de abertura. Parser mínimo: aceita aspas simples,
/// duplas e valores sem aspas, que é o que HTML real de relatório produz.
fn atributos_da_tag(bloco: &str) -> Vec<(String, String)> {
    let fim = bloco.find('>').unwrap_or(bloco.len());
    let corpo = &bloco[4..fim]; // pula "<svg"
    let bytes: Vec<char> = corpo.chars().collect();
    let mut pares = Vec::new();
    let mut i = 0;

    while i < bytes.len() {
        while i < bytes.len() && (bytes[i].is_whitespace() || bytes[i] == '/') {
            i += 1;
        }
        let inicio_nome = i;
        while i < bytes.len() && !bytes[i].is_whitespace() && bytes[i] != '=' && bytes[i] != '/' {
            i += 1;
        }
        if i == inicio_nome {
            break;
        }
        let nome: String = bytes[inicio_nome..i].iter().collect();

        while i < bytes.len() && bytes[i].is_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != '=' {
            pares.push((nome, String::new()));
            continue;
        }
        i += 1; // '='
        while i < bytes.len() && bytes[i].is_whitespace() {
            i += 1;
        }

        let valor = if i < bytes.len() && (bytes[i] == '"' || bytes[i] == '\'') {
            let aspa = bytes[i];
            i += 1;
            let inicio = i;
            while i < bytes.len() && bytes[i] != aspa {
                i += 1;
            }
            let v: String = bytes[inicio..i].iter().collect();
            i += 1; // fecha aspa
            v
        } else {
            let inicio = i;
            while i < bytes.len() && !bytes[i].is_whitespace() {
                i += 1;
            }
            bytes[inicio..i].iter().collect()
        };
        pares.push((nome, valor));
    }

    pares
}

fn escapar_aspas(valor: &str) -> String {
    valor.replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SVG: &str = r##"<svg width="20" height="10" xmlns="http://www.w3.org/2000/svg"><rect width="20" height="10" fill="#f00"/></svg>"##;

    fn arvore(fonte: &str) -> usvg::Tree {
        usvg::Tree::from_data(fonte.as_bytes(), &usvg::Options::default()).unwrap()
    }

    #[test]
    fn rasteriza_com_superamostragem() {
        let (w, h, rgba) = rasterizar(&arvore(SVG), 20.0, 10.0).unwrap();
        assert_eq!((w, h), (60, 30), "esperava 3x o tamanho da caixa");
        assert_eq!(rgba.len() as u32, w * h * 4);
        // Pixel do meio é o vermelho do rect, opaco.
        let meio = ((h / 2 * w + w / 2) * 4) as usize;
        assert_eq!(&rgba[meio..meio + 4], &[255, 0, 0, 255]);
    }

    #[test]
    fn area_transparente_sai_com_alfa_zero() {
        let fonte = r##"<svg width="10" height="10" xmlns="http://www.w3.org/2000/svg"><rect x="0" y="0" width="2" height="2" fill="#00f"/></svg>"##;
        let (w, h, rgba) = rasterizar(&arvore(fonte), 10.0, 10.0).unwrap();
        let canto = ((h - 1) * w + (w - 1)) as usize * 4;
        assert_eq!(rgba[canto + 3], 0, "canto vazio devia ser transparente");
    }

    #[test]
    fn caixa_gigante_respeita_o_teto() {
        let (w, h, _) = rasterizar(&arvore(SVG), 8000.0, 4000.0).unwrap();
        // 8000px de caixa a 3x daria 24000: a escala cai para caber em 4096, e
        // a proporção é preservada.
        assert_eq!((w, h), (LADO_MAXIMO_PX, LADO_MAXIMO_PX / 2), "{w}x{h}");
    }

    #[test]
    fn caixa_degenerada_nao_panica() {
        assert!(rasterizar(&arvore(SVG), 0.0, 10.0).is_none());
        assert!(rasterizar(&arvore(SVG), f32::NAN, 10.0).is_none());
        assert!(rasterizar(&arvore(SVG), -5.0, 10.0).is_none());
    }

    #[test]
    fn svg_inline_vira_img_data_uri() {
        let saida = reescrever_svg_inline(&format!("<p>antes</p>{SVG}<p>depois</p>"));
        assert!(saida.starts_with("<p>antes</p><img "), "{saida}");
        assert!(saida.ends_with("<p>depois</p>"), "{saida}");
        assert!(saida.contains("src=\"data:image/svg+xml;base64,"), "{saida}");
    }

    #[test]
    fn atributos_de_apresentacao_passam_para_o_img() {
        let entrada = r##"<svg class="logo" style="height:150px" width="20" height="10" fill="#f00"></svg>"##;
        let saida = reescrever_svg_inline(entrada);
        assert!(saida.contains(r#"class="logo""#), "{saida}");
        assert!(saida.contains(r#"style="height:150px""#), "{saida}");
        assert!(saida.contains(r#"width="20""#), "{saida}");
        assert!(!saida.contains("fill="), "fill é do SVG, não do img: {saida}");
    }

    #[test]
    fn svg_aninhado_fecha_no_nivel_certo() {
        let entrada = r#"<svg width="10" height="10"><svg width="5" height="5"></svg></svg>FIM"#;
        let saida = reescrever_svg_inline(entrada);
        assert!(saida.ends_with("FIM"), "{saida}");
        assert_eq!(saida.matches("<img").count(), 1, "aninhado virou dois img: {saida}");
    }

    #[test]
    fn svg_auto_fechado_vira_um_img() {
        let saida = reescrever_svg_inline(r#"<svg width="10" height="10"/>FIM"#);
        assert_eq!(saida.matches("<img").count(), 1, "{saida}");
        assert!(saida.ends_with("FIM"), "{saida}");
    }

    #[test]
    fn texto_sem_svg_passa_intacto() {
        let entrada = "<p>svgnao</p><svgfoo>x</svgfoo>";
        assert_eq!(reescrever_svg_inline(entrada), entrada);
    }

    #[test]
    fn svg_sem_fechamento_nao_perde_conteudo() {
        let entrada = "<p>a</p><svg width=\"10\"><rect/>";
        assert_eq!(reescrever_svg_inline(entrada), entrada);
    }
}
