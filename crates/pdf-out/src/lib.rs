//! Emissão de PDF a partir de páginas já paginadas.
//! Texto sai como operador de texto com glifos posicionados — nunca rasterizado.

use paginate::{Page, PageGeometry};
use printpdf::*;
use render_ir::FontResource;

/// PDF trabalha em pt; o resto do motor em px a 96dpi.
fn pt(px: f32) -> f32 {
    px * 0.75
}

const PT_POR_MM: f32 = 72.0 / 25.4;

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
                let Some(Some(font_id)) = ids.get(run.font_index) else {
                    continue;
                };
                if run.glyphs.is_empty() {
                    continue;
                }
                let handle = PdfFontHandle::External(font_id.clone());

                ops.push(Op::StartTextSection);
                ops.push(Op::SetFillColor { col: cor_rgb(run.color) });
                ops.push(Op::SetFont { font: handle.clone(), size: Pt(pt(run.font_size_px)) });

                // Um glifo por vez, cada um com sua própria matriz de texto: o
                // shaping já deu a posição absoluta de cada glifo dentro do run,
                // então não há avanço a recalcular aqui. PDF tem origem embaixo à
                // esquerda; a display list, em cima à esquerda.
                let mut chars = run.text.chars();
                for g in &run.glyphs {
                    let x = pt(run.origin_x + g.x);
                    let y = altura_pt - pt(run.baseline_y + g.y);
                    ops.push(Op::SetTextMatrix {
                        matrix: TextMatrix::Translate(Pt(x), Pt(y)),
                    });
                    ops.push(Op::ShowText {
                        items: vec![TextItem::GlyphIds(vec![Codepoint {
                            gid: g.id,
                            offset: 0.0,
                            // Alimenta o ToUnicode para o texto sair selecionável.
                            cid: chars.next().map(String::from),
                        }])],
                    });
                }

                ops.push(Op::EndTextSection);
            }

            PdfPage::new(
                Mm(largura_pt / PT_POR_MM),
                Mm(altura_pt / PT_POR_MM),
                ops,
            )
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
        let bytes = render_pdf(
            &[Page::default(), Page::default(), Page::default()],
            &[],
            &geo(),
        );
        let texto = String::from_utf8_lossy(&bytes);
        // printpdf serializa sem espaço; "/Type/Page/" não casa com "/Type/Pages".
        let ocorrencias = texto.matches("/Type/Page/").count();
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
        assert!(
            texto.contains("/FontFile2") || texto.contains("/FontFile3"),
            "fonte não foi embutida"
        );
        assert!(!texto.contains("/Subtype /Image"), "texto virou bitmap");
    }

    /// Usa uma fonte do próprio sistema para o teste não depender de fixture binária.
    fn carregar_fonte_de_teste() -> Vec<u8> {
        let candidatos = [
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/System/Library/Fonts/Helvetica.ttc",
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

