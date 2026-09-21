//! Dá layout ao HTML com o blitz e extrai uma display list própria.
//! Esta é a única crate que conhece os tipos do blitz — a fronteira existe para
//! que trocar o miolo de layout não se propague para paginate e pdf-out.
//!
//! **Pressupõe `scale = 1.0`.** O `parley` trabalha em px de dispositivo e o
//! blitz divide as medidas pelo `scale` ao montar o layout
//! (`blitz-dom/src/layout/inline.rs`). Como o viewport é criado com escala 1.0,
//! px de dispositivo e px CSS coincidem e a extração pode misturar as duas
//! fontes de coordenada sem conversão. Mudar a escala exige revisar isto.

use crate::net::{ColetorDeRecursos, ProvedorDataUri};
use blitz_dom::DocumentConfig;
use blitz_html::HtmlDocument;
use blitz_traits::shell::{ColorScheme, Viewport};
use render_ir::{BoxItem, DisplayList, FontResource, Glyph, ImageItem, Rect, TextRun};
use std::sync::Arc;
use style::properties::ComputedValues;

/// Altura inicial do viewport de layout. O documento é contínuo: a altura real
/// sai de `DisplayList::content_height`, e o corte em páginas é de `paginate`.
const ALTURA_INICIAL_PX: u32 = 20_000;

/// Teto rígido para o crescimento do viewport. Alcançá-lo é um erro de projeto,
/// não um caso normal — por isso é avisado em `stderr` em vez de truncar calado.
const ALTURA_MAXIMA_PX: u32 = 400_000;

pub fn render_html(html: &str, width_px: f32) -> DisplayList {
    // O viewport é inteiro: arredondar (e não truncar) evita perder a última
    // coluna de px em larguras como 595.28 (A4). O mínimo de 1 também absorve
    // largura negativa ou NaN, que saturariam em 0.
    let largura_viewport = width_px.round().max(1.0) as u32;

    // O blitz só entende SVG que chega como recurso; `<svg>` escrito no HTML
    // vira caixa vazia. A reescrita normaliza isso antes do parse.
    let html = &crate::svg::reescrever_svg_inline(html);

    let mut altura = ALTURA_INICIAL_PX;
    loop {
        let (dl, altura_usada) = layout_e_extrai(html, width_px, largura_viewport, altura);

        if altura_usada <= altura as f32 {
            return dl;
        }
        if altura >= ALTURA_MAXIMA_PX {
            eprintln!(
                "render-core: conteúdo de {altura_usada:.0}px excede o teto de \
                 layout de {ALTURA_MAXIMA_PX}px; o excedente foi truncado"
            );
            return dl;
        }
        altura = altura.saturating_mul(2).min(ALTURA_MAXIMA_PX);
    }
}

/// Dá um layout com a altura de viewport pedida e devolve a display list junto
/// da altura que o conteúdo realmente ocupou (base para decidir se cresce).
fn layout_e_extrai(
    html: &str,
    width_px: f32,
    largura_viewport: u32,
    altura_viewport: u32,
) -> (DisplayList, f32) {
    // O provedor resolve `data:` de forma síncrona e recusa qualquer outro
    // esquema; o coletor guarda o que foi resolvido para aplicarmos abaixo.
    let coletor = Arc::new(ColetorDeRecursos::default());
    let mut doc = HtmlDocument::from_html(
        html,
        DocumentConfig {
            net_provider: Some(Arc::new(ProvedorDataUri::new(coletor.clone()))),
            ..Default::default()
        },
    );
    doc.set_viewport(Viewport::new(
        largura_viewport,
        altura_viewport,
        1.0,
        ColorScheme::Light,
    ));
    doc.resolve(0.0);

    // As imagens só existem depois que o recurso entra no documento, e isso
    // invalida o layout — daí o segundo resolve.
    let recursos = coletor.drenar();
    let havia_recursos = !recursos.is_empty();
    for recurso in recursos {
        doc.load_resource(recurso);
    }
    if havia_recursos {
        doc.resolve(0.0);
    }

    let mut dl = DisplayList {
        width: width_px,
        ..Default::default()
    };
    let mut fontes: Vec<FontResource> = Vec::new();

    let tree = doc.tree();

    for (_id, node) in tree.iter() {
        // `final_layout.location` é relativo ao pai; a display list é em
        // coordenadas do documento.
        let pos = node.absolute_position(0.0, 0.0);
        let layout = &node.final_layout;
        let (x, y) = (pos.x, pos.y);

        let Some(el) = node.element_data() else {
            continue;
        };

        let estilo = node.primary_styles();
        if let Some(s) = estilo.as_deref() {
            let fundo = cor_de_fundo(s);
            let (borda_cor, borda_largura) = borda_da_caixa(s);

            // Um nó sem fundo visível e sem borda não pinta nada: emitir uma
            // caixa aqui encheria a lista de `<html>`, `<head>` e `<style>`.
            if fundo.is_some() || borda_largura > 0.0 {
                dl.boxes.push(BoxItem {
                    rect: Rect {
                        x,
                        y,
                        width: layout.size.width,
                        height: layout.size.height,
                    },
                    background: fundo,
                    border_color: borda_cor,
                    border_width: borda_largura,
                });
            }
        }

        let caixa_da_imagem = Rect {
            x,
            y,
            width: layout.size.width,
            height: layout.size.height,
        };
        if let Some(raster) = el.raster_image_data() {
            dl.images.push(ImageItem {
                rect: caixa_da_imagem,
                width_px: raster.width,
                height_px: raster.height,
                rgba: raster.data.clone(),
            });
        } else if let Some(tree) = el.svg_data() {
            // SVG é vetorial, mas o resto do pipeline (paginate, pdf-out) só
            // conhece bitmap: rasterizamos no tamanho final da caixa.
            if let Some((largura, altura, rgba)) =
                crate::svg::rasterizar(tree, caixa_da_imagem.width, caixa_da_imagem.height)
            {
                dl.images.push(ImageItem {
                    rect: caixa_da_imagem,
                    width_px: largura,
                    height_px: altura,
                    rgba: Arc::new(rgba),
                });
            }
        }

        let Some(text_layout) = el.inline_layout_data.as_ref() else {
            continue;
        };

        // O layout do parley é ancorado no content box; `absolute_position` dá o
        // border box. O mesmo ajuste que o blitz faz em `Node::hit`.
        let texto_x = x + layout.padding.left + layout.border.left;
        let texto_y = y + layout.padding.top + layout.border.top;

        for line in text_layout.layout.lines() {
            for item in line.items() {
                let parley::PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                    continue;
                };
                let run = glyph_run.run();
                let font_index = indice_da_fonte(&mut fontes, run.font());
                let tamanho = run.font_size();

                let deslocamento = glyph_run.offset();
                let baseline = glyph_run.baseline();

                // `glyphs()` devolve só o offset do shaper, sem acumular
                // `advance` — todos os glifos cairiam na mesma abscissa.
                // `positioned_glyphs()` acumula, mas já soma `offset`/`baseline`
                // do run; descontamos os dois para manter o contrato do
                // `render_ir::Glyph` (posição relativa à origem do run) sem
                // contar duas vezes.
                let glifos: Vec<Glyph> = glyph_run
                    .positioned_glyphs()
                    .map(|g| Glyph {
                        id: g.id as u16,
                        x: g.x - deslocamento,
                        y: g.y - baseline,
                    })
                    .collect();
                if glifos.is_empty() {
                    continue;
                }

                let faixa = run.text_range();
                let fatia = text_layout.text.get(faixa.start..faixa.end);
                debug_assert!(
                    fatia.is_some(),
                    "text_range {faixa:?} fora de char boundary do texto do nó; \
                     o ToUnicode do PDF ficaria vazio sem aviso"
                );

                dl.texts.push(TextRun {
                    origin_x: texto_x + deslocamento,
                    baseline_y: texto_y + baseline,
                    font_index,
                    font_size_px: tamanho,
                    color: cor_do_brush(tree, glyph_run.style().brush.id),
                    glyphs: glifos,
                    text: fatia.unwrap_or_default().to_string(),
                });
            }
        }
    }

    dl.fonts = fontes;

    let raiz = doc.root_element().final_layout;
    // A raiz sabe a altura reservada pelo layout; a display list, só o que foi
    // pintado. Guardar as duas deixa o screenshot enxergar espaço em branco
    // deliberado sem que a paginação perca conteúdo desenhado fora da raiz.
    dl.layout_height = raiz.size.height.max(raiz.content_size.height);
    let altura_usada = dl.content_height();

    (dl, altura_usada)
}

/// Guarda a fonte uma única vez e devolve seu índice em `DisplayList::fonts`.
fn indice_da_fonte(fontes: &mut Vec<FontResource>, font: &parley::FontData) -> usize {
    let bytes: &[u8] = font.data.as_ref();
    let face = font.index as usize;
    if let Some(pos) = fontes
        .iter()
        .position(|f| f.face_index == face && f.bytes.len() == bytes.len() && f.bytes == bytes)
    {
        return pos;
    }
    fontes.push(FontResource {
        bytes: bytes.to_vec(),
        face_index: face,
    });
    fontes.len() - 1
}

/// Cor de fundo, ou `None` quando não há nada visível para pintar.
///
/// O valor inicial de `background-color` é `transparent`, que no stylo é a
/// variante `Absolute` (preto com alfa 0). Ler só os componentes RGB faria todo
/// elemento virar uma caixa preta opaca.
fn cor_de_fundo(estilo: &ComputedValues) -> Option<[u8; 3]> {
    let cor = estilo.get_background().background_color.as_absolute()?;
    if cor.alpha <= 0.0 {
        return None;
    }
    Some(rgb(cor))
}

/// Cor e largura da borda de topo. `render_ir::BoxItem` só comporta uma borda,
/// então bordas assimétricas são representadas pela de topo.
fn borda_da_caixa(estilo: &ComputedValues) -> (Option<[u8; 3]>, f32) {
    let borda = estilo.get_border();
    let largura = borda.border_top_width.to_f32_px();
    if largura <= 0.0 {
        return (None, 0.0);
    }
    // O valor inicial de `border-color` é `currentcolor`, que não é `Absolute`:
    // resolver pela propriedade `color` do próprio elemento.
    let cor = borda
        .border_top_color
        .as_absolute()
        .copied()
        .unwrap_or_else(|| estilo.get_inherited_text().clone_color());
    (Some(rgb(&cor)), largura)
}

/// O `TextBrush` do blitz não carrega cor: é o id do nó do span que originou o
/// run (`blitz-dom/src/node/element.rs`). A cor sai da propriedade `color`
/// computada desse nó.
fn cor_do_brush(tree: &slab::Slab<blitz_dom::Node>, id: usize) -> [u8; 3] {
    tree.get(id)
        .and_then(|n| n.primary_styles())
        .map(|s| rgb(&s.get_inherited_text().clone_color()))
        .unwrap_or([0, 0, 0])
}

fn rgb(c: &style::color::AbsoluteColor) -> [u8; 3] {
    let srgb = c.to_color_space(style::color::ColorSpace::Srgb);
    [
        (srgb.components.0 * 255.0).round().clamp(0.0, 255.0) as u8,
        (srgb.components.1 * 255.0).round().clamp(0.0, 255.0) as u8,
        (srgb.components.2 * 255.0).round().clamp(0.0, 255.0) as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;

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

    // --- Asserções somadas na rodada de correção 1 ---

    #[test]
    fn glifos_de_um_run_avancam_horizontalmente() {
        let dl = render_html(HTML_PARAGRAFO, 643.0);
        assert!(!dl.texts.is_empty(), "nenhum run extraído");
        let run = dl
            .texts
            .iter()
            .find(|r| r.glyphs.len() >= 2)
            .expect("esperava um run com ao menos dois glifos");
        for par in run.glyphs.windows(2) {
            assert!(
                par[1].x > par[0].x,
                "glifos não avançam: {:?} depois de {:?}",
                par[1],
                par[0]
            );
        }
    }

    #[test]
    fn posicoes_sao_absolutas_no_documento() {
        let html = r#"<!DOCTYPE html><html><head><style>
            body { margin: 0; font-size: 16px; }
            </style></head><body>
            <div style="margin-top:500px;background:#ff0000">Empurrado</div>
            </body></html>"#;
        let dl = render_html(html, 600.0);

        let caixa = dl
            .boxes
            .iter()
            .find(|b| b.background == Some([255, 0, 0]))
            .expect("esperava a caixa vermelha");
        assert!(caixa.rect.y >= 495.0, "rect.y={} não é absoluto", caixa.rect.y);

        assert!(!dl.texts.is_empty(), "nenhum run extraído");
        for run in &dl.texts {
            assert!(
                run.baseline_y >= 495.0,
                "baseline_y={} não é absoluto",
                run.baseline_y
            );
        }
    }

    #[test]
    fn html_sem_fundo_declarado_nao_gera_caixa_alguma() {
        let dl = render_html(HTML_PARAGRAFO, 643.0);
        assert!(
            dl.boxes.is_empty(),
            "fundo transparente virou caixa: {:?}",
            dl.boxes
        );
        assert!(!dl.texts.is_empty(), "nenhum run extraído");
    }

    #[test]
    fn head_e_style_nao_viram_caixas_mesmo_com_fundo_no_body() {
        let html = r#"<!DOCTYPE html><html><head><style>
            body { margin: 0; background: #ffffff; }
            </style></head><body><p>Oi</p></body></html>"#;
        let dl = render_html(html, 600.0);
        assert_eq!(
            dl.boxes.len(),
            1,
            "só o body devia pintar, veio {:?}",
            dl.boxes
        );
        assert_eq!(dl.boxes[0].background, Some([255, 255, 255]));
    }

    #[test]
    fn cor_do_texto_vem_do_estilo_computado() {
        let html = r#"<!DOCTYPE html><html><head><style>
            body { margin: 0; font-size: 16px; }
            </style></head><body>
            <p style="color:#ff0000">Vermelho</p>
            </body></html>"#;
        let dl = render_html(html, 600.0);
        assert!(!dl.texts.is_empty(), "nenhum run extraído");
        assert!(
            dl.texts.iter().any(|r| r.color == [255, 0, 0]),
            "esperava um run vermelho, veio {:?}",
            dl.texts.iter().map(|r| r.color).collect::<Vec<_>>()
        );
    }

    #[test]
    fn viewport_cresce_para_conteudo_alto() {
        let html = r#"<!DOCTYPE html><html><head><style>
            body { margin: 0; }
            </style></head><body>
            <div style="height:30000px"></div>
            <div style="height:20px;background:#00ff00">fim</div>
            </body></html>"#;
        let dl = render_html(html, 600.0);
        let verde = dl
            .boxes
            .iter()
            .find(|b| b.background == Some([0, 255, 0]))
            .expect("a caixa após 30000px sumiu — o viewport não cresceu");
        assert!(
            verde.rect.y >= 30000.0,
            "rect.y={} — conteúdo foi truncado pelo viewport inicial",
            verde.rect.y
        );
        assert!(dl.content_height() > 20_000.0);
    }
    /// PNG 4x2 vermelho, embutido para o teste não depender de fixture binária.
    const PNG_4X2: &str = "iVBORw0KGgoAAAANSUhEUgAAAAQAAAACCAIAAADwyuo0AAAAEElEQVR4nGM4IScHRwzIHABvCgghBqXSdgAAAABJRU5ErkJggg==";

    #[test]
    fn imagem_data_uri_vira_item_da_display_list() {
        let html = format!(
            r#"<img style="width:80px;height:40px" src="data:image/png;base64,{PNG_4X2}" />"#
        );
        let dl = render_html(&html, 500.0);
        assert_eq!(dl.images.len(), 1, "a imagem não chegou na display list");
        let img = &dl.images[0];
        assert_eq!((img.width_px, img.height_px), (4, 2), "tamanho do bitmap");
        assert!((img.rect.width - 80.0).abs() < 1.0, "largura da caixa: {:?}", img.rect);
        assert!((img.rect.height - 40.0).abs() < 1.0, "altura da caixa: {:?}", img.rect);
        assert_eq!(img.rgba.len(), 4 * 2 * 4, "RGBA8 de 4x2");
    }

    #[test]
    fn imagem_sem_dimensao_explicita_usa_o_tamanho_intrinseco() {
        let html = format!(r#"<img src="data:image/png;base64,{PNG_4X2}" />"#);
        let dl = render_html(&html, 500.0);
        let img = &dl.images[0];
        assert!((img.rect.width - 4.0).abs() < 1.0, "largura intrínseca: {:?}", img.rect);
        assert!((img.rect.height - 2.0).abs() < 1.0, "altura intrínseca: {:?}", img.rect);
    }

    const SVG_20X10: &str = r##"<svg width="20" height="10" xmlns="http://www.w3.org/2000/svg"><rect width="20" height="10" fill="#0000ff"/></svg>"##;

    #[test]
    fn svg_em_img_data_uri_vira_item_da_display_list() {
        let b64 = base64::engine::general_purpose::STANDARD.encode(SVG_20X10);
        let html = format!(r#"<img style="width:40px;height:20px" src="data:image/svg+xml;base64,{b64}" />"#);
        let dl = render_html(&html, 500.0);
        assert_eq!(dl.images.len(), 1, "o SVG não chegou na display list");
        let img = &dl.images[0];
        assert!((img.rect.width - 40.0).abs() < 1.0, "caixa: {:?}", img.rect);
        // Rasterizado em 3x a caixa, não no tamanho intrínseco do viewBox.
        assert_eq!((img.width_px, img.height_px), (120, 60));
        assert_eq!(img.rgba.len(), 120 * 60 * 4);
    }

    #[test]
    fn svg_sem_dimensao_usa_o_tamanho_intrinseco() {
        let b64 = base64::engine::general_purpose::STANDARD.encode(SVG_20X10);
        let html = format!(r#"<img src="data:image/svg+xml;base64,{b64}" />"#);
        let dl = render_html(&html, 500.0);
        let img = &dl.images[0];
        assert!((img.rect.width - 20.0).abs() < 1.0, "largura intrínseca: {:?}", img.rect);
        assert!((img.rect.height - 10.0).abs() < 1.0, "altura intrínseca: {:?}", img.rect);
    }

    #[test]
    fn svg_inline_vira_imagem_com_tamanho_intrinseco() {
        let dl = render_html(&format!("<body style=\"margin:0\">{SVG_20X10}</body>"), 500.0);
        assert_eq!(dl.images.len(), 1, "o <svg> inline não virou imagem");
        let img = &dl.images[0];
        assert!((img.rect.width - 20.0).abs() < 1.0, "caixa: {:?}", img.rect);
        assert!((img.rect.height - 10.0).abs() < 1.0, "caixa: {:?}", img.rect);
    }

    #[test]
    fn svg_inline_respeita_css_da_tag() {
        let com_estilo = SVG_20X10.replace("<svg ", r#"<svg style="width:100px;height:50px" "#);
        let dl = render_html(&format!("<body style=\"margin:0\">{com_estilo}</body>"), 500.0);
        let img = &dl.images[0];
        assert!((img.rect.width - 100.0).abs() < 1.0, "caixa: {:?}", img.rect);
        assert!((img.rect.height - 50.0).abs() < 1.0, "caixa: {:?}", img.rect);
    }

    #[test]
    fn svg_em_http_e_ignorado_sem_abrir_socket() {
        let dl = render_html(r#"<img src="https://exemplo.invalido/logo.svg" />"#, 500.0);
        assert!(dl.images.is_empty(), "esquema não-data: não pode virar imagem");
    }

    #[test]
    fn imagem_http_e_ignorada_sem_abrir_socket() {
        // Invariante de segurança: só data: é resolvido.
        let dl = render_html(r#"<img src="https://exemplo.invalido/logo.png" />"#, 500.0);
        assert!(dl.images.is_empty(), "esquema não-data: não pode virar imagem");
    }
}
