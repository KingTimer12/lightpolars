//! Corte de um documento contínuo em páginas.
//! Aritmética pura sobre retângulos: não renderiza nada e não conhece blitz.

use render_ir::{BoxItem, DisplayList, FontResource, ImageItem, TextRun};

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
    pub images: Vec<ImageItem>,
}

/// Tabela de fontes única para o documento inteiro, na mesma ordem que
/// `paginate` usa ao reindexar os runs: conteúdo, depois header, depois footer.
/// `pdf_out::render_pdf` deve receber exatamente esta lista.
pub fn merge_fonts(
    content: &DisplayList,
    header: Option<&DisplayList>,
    footer: Option<&DisplayList>,
) -> Vec<FontResource> {
    let mut fonts = content.fonts.clone();
    for extra in [header, footer].into_iter().flatten() {
        fonts.extend(extra.fonts.iter().cloned());
    }
    fonts
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

    // Offsets casados com a ordem de `merge_fonts`.
    let offset_header = content.fonts.len();
    let offset_footer = offset_header + header.map_or(0, |h| h.fonts.len());

    let cortes = calcular_cortes(content, altura_util);
    let mut paginas = Vec::with_capacity(cortes.len());

    for faixa in &cortes {
        let mut page = Page::default();

        if let Some(h) = header {
            empilhar(&mut page, h, geo.margins.left, 0.0, offset_header);
        }
        if let Some(f) = footer {
            let base = geo.sheet_height - geo.margins.bottom;
            empilhar(&mut page, f, geo.margins.left, base, offset_footer);
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
        for i in &content.images {
            if i.rect.y >= faixa.inicio && i.rect.y < faixa.fim {
                let mut novo = i.clone();
                novo.rect.x += geo.margins.left;
                novo.rect.y += deslocamento;
                page.images.push(novo);
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
    let retangulos = content
        .boxes
        .iter()
        .map(|b| b.rect)
        .chain(content.images.iter().map(|i| i.rect));
    for r in retangulos {
        let topo = r.y;
        let base = r.bottom();
        // Caixa atravessa o corte: empurra a caixa inteira para a página seguinte.
        if topo > inicio && topo < corte && base > corte {
            corte = corte.min(topo);
        }
    }
    corte
}

fn empilhar(page: &mut Page, fonte: &DisplayList, dx: f32, dy: f32, offset_fonte: usize) {
    for i in &fonte.images {
        let mut novo = i.clone();
        novo.rect.x += dx;
        novo.rect.y += dy;
        page.images.push(novo);
    }
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
        novo.font_index += offset_fonte;
        page.texts.push(novo);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use render_ir::{BoxItem, DisplayList, FontResource, Glyph, ImageItem, Rect, TextRun};

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
        dl.texts.push(texto_em(h + 10.0));
        let pages = paginate(&dl, None, None, &geo);
        assert_eq!(pages.len(), 2);
        let run = &pages[1].texts[0];
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
        dl.boxes.push(BoxItem {
            rect: Rect { x: 0.0, y: h - 20.0, width: 100.0, height: 60.0 },
            background: None,
            border_color: Some([0, 0, 0]),
            border_width: 1.0,
        });
        let pages = paginate(&dl, None, None, &geo);
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].boxes.len(), 0);
        assert_eq!(pages[1].boxes.len(), 1);
    }

    fn fonte(marca: u8) -> FontResource {
        FontResource { bytes: vec![marca], face_index: 0 }
    }

    #[test]
    fn merge_fonts_concatena_conteudo_header_footer_nessa_ordem() {
        let mut content = DisplayList::default();
        content.fonts.push(fonte(1));
        let mut header = DisplayList::default();
        header.fonts.push(fonte(2));
        let mut footer = DisplayList::default();
        footer.fonts.push(fonte(3));

        let fonts = merge_fonts(&content, Some(&header), Some(&footer));
        assert_eq!(fonts.len(), 3);
        assert_eq!(fonts[0].bytes, vec![1]);
        assert_eq!(fonts[1].bytes, vec![2]);
        assert_eq!(fonts[2].bytes, vec![3]);
    }

    #[test]
    fn font_index_de_header_e_footer_e_reindexado_para_a_tabela_unica() {
        let geo = PageGeometry::a4(false, Margins {
            top: 100.0, right: 0.0, bottom: 100.0, left: 0.0,
        });
        let mut content = DisplayList::default();
        content.fonts.push(fonte(1));
        content.texts.push(texto_em(10.0));

        let mut header = DisplayList::default();
        header.fonts.push(fonte(2));
        header.texts.push(texto_em(20.0)); // font_index 0 na lista do header

        let mut footer = DisplayList::default();
        footer.fonts.push(fonte(3));
        footer.texts.push(texto_em(5.0));

        let pages = paginate(&content, Some(&header), Some(&footer), &geo);
        assert_eq!(pages.len(), 1);
        let idx: Vec<usize> = pages[0].texts.iter().map(|t| t.font_index).collect();
        // header, footer, conteúdo — na ordem em que paginate empilha.
        assert_eq!(idx, vec![1, 2, 0]);

        let fonts = merge_fonts(&content, Some(&header), Some(&footer));
        assert_eq!(fonts[idx[0]].bytes, vec![2], "header aponta para a fonte do header");
        assert_eq!(fonts[idx[1]].bytes, vec![3], "footer aponta para a fonte do footer");
        assert_eq!(fonts[idx[2]].bytes, vec![1], "conteúdo aponta para a própria fonte");
    }
    fn imagem_em(y: f32, altura: f32) -> ImageItem {
        ImageItem {
            rect: Rect { x: 0.0, y, width: 100.0, height: altura },
            width_px: 10,
            height_px: 10,
            rgba: std::sync::Arc::new(vec![0; 10 * 10 * 4]),
        }
    }

    #[test]
    fn imagem_vai_para_a_pagina_certa_com_as_margens_aplicadas() {
        let geo = PageGeometry::a4(false, Margins {
            top: 100.0, right: 0.0, bottom: 0.0, left: 50.0,
        });
        let h = geo.content_height();
        let mut dl = DisplayList::default();
        dl.images.push(imagem_em(10.0, 20.0));
        dl.images.push(imagem_em(h + 10.0, 20.0));

        let pages = paginate(&dl, None, None, &geo);
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].images.len(), 1);
        assert_eq!(pages[1].images.len(), 1);
        assert!((pages[1].images[0].rect.y - 110.0).abs() < 0.001);
        assert!((pages[1].images[0].rect.x - 50.0).abs() < 0.001);
    }

    #[test]
    fn corte_nao_parte_uma_imagem_ao_meio() {
        let geo = PageGeometry::a4(false, margens_zero());
        let h = geo.content_height();
        let mut dl = DisplayList::default();
        dl.images.push(imagem_em(h - 20.0, 60.0));

        let pages = paginate(&dl, None, None, &geo);
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].images.len(), 0, "imagem cortada ao meio");
        assert_eq!(pages[1].images.len(), 1);
    }

    #[test]
    fn imagem_de_header_e_repetida_em_todas_as_paginas() {
        let geo = PageGeometry::a4(false, Margins {
            top: 100.0, right: 0.0, bottom: 0.0, left: 0.0,
        });
        let h = geo.content_height();
        let mut dl = DisplayList::default();
        dl.texts.push(texto_em(10.0));
        dl.texts.push(texto_em(h + 10.0));

        let mut header = DisplayList::default();
        header.images.push(imagem_em(10.0, 50.0));

        let pages = paginate(&dl, Some(&header), None, &geo);
        assert_eq!(pages.len(), 2);
        for page in &pages {
            assert_eq!(page.images.len(), 1, "logo do header some numa das páginas");
        }
    }
}
