//! Dá layout ao HTML com o blitz e extrai uma display list própria.
//! Esta é a única crate que conhece os tipos do blitz — a fronteira existe para
//! que trocar o miolo de layout não se propague para paginate e pdf-out.

use blitz_dom::DocumentConfig;
use blitz_html::HtmlDocument;
use blitz_traits::shell::{ColorScheme, Viewport};
use render_ir::{BoxItem, DisplayList, FontResource, Glyph, Rect, TextRun};
use style::properties::ComputedValues;

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

    let mut dl = DisplayList {
        width: width_px,
        ..Default::default()
    };
    let mut fontes: Vec<FontResource> = Vec::new();

    for (_id, node) in doc.tree().iter() {
        // `final_layout.location` é relativo ao pai; a display list é em
        // coordenadas do documento.
        let pos = node.absolute_position(0.0, 0.0);
        let (x, y) = (pos.x, pos.y);
        let size = node.final_layout.size;

        let Some(el) = node.element_data() else {
            continue;
        };

        let estilo = node.primary_styles();
        let (fundo, borda_cor, borda_largura) = match estilo.as_deref() {
            Some(s) => cores_da_caixa(s),
            None => (None, None, 0.0),
        };

        if fundo.is_some() || borda_largura > 0.0 {
            dl.boxes.push(BoxItem {
                rect: Rect {
                    x,
                    y,
                    width: size.width,
                    height: size.height,
                },
                background: fundo,
                border_color: borda_cor,
                border_width: borda_largura,
            });
        }

        let Some(text_layout) = el.inline_layout_data.as_ref() else {
            continue;
        };

        for line in text_layout.layout.lines() {
            for item in line.items() {
                let parley::PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                    continue;
                };
                let run = glyph_run.run();
                let font_index = indice_da_fonte(&mut fontes, run.font());
                let tamanho = run.font_size();

                let glifos: Vec<Glyph> = glyph_run
                    .glyphs()
                    .map(|g| Glyph {
                        id: g.id as u16,
                        x: g.x,
                        y: g.y,
                    })
                    .collect();
                if glifos.is_empty() {
                    continue;
                }

                let faixa = run.text_range();
                let texto = text_layout
                    .text
                    .get(faixa.start..faixa.end)
                    .unwrap_or_default()
                    .to_string();

                dl.texts.push(TextRun {
                    origin_x: x + glyph_run.offset(),
                    baseline_y: y + glyph_run.baseline(),
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

fn cores_da_caixa(estilo: &ComputedValues) -> (Option<[u8; 3]>, Option<[u8; 3]>, f32) {
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
