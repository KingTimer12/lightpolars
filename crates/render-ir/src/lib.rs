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

/// Uma imagem já decodificada, posicionada pelo layout.
/// Guarda RGBA8 porque é o que o blitz entrega e o que o PDF consome; o Arc
/// existe para o corte em páginas não copiar o bitmap inteiro.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageItem {
    pub rect: Rect,
    /// Largura e altura em pixels do bitmap, não da caixa onde ele é desenhado.
    pub width_px: u32,
    pub height_px: u32,
    pub rgba: std::sync::Arc<Vec<u8>>,
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
    pub images: Vec<ImageItem>,
    pub fonts: Vec<FontResource>,
    pub width: f32,
    /// Altura que o layout reservou para a raiz do documento, pintada ou não.
    ///
    /// Existe porque um elemento invisível (um `<div style="height:3000px">`
    /// sem fundo) não emite item nenhum: só pelos itens desenhados o documento
    /// pareceria ter altura zero, e o screenshot sairia recortado.
    pub layout_height: f32,
}

impl DisplayList {
    /// Altura ocupada pelo conteúdo. Base do corte de páginas.
    pub fn content_height(&self) -> f32 {
        let from_boxes = self.boxes.iter().map(|b| b.rect.bottom());
        let from_texts = self.texts.iter().map(|t| t.baseline_y);
        let from_images = self.images.iter().map(|i| i.rect.bottom());
        from_boxes
            .chain(from_texts)
            .chain(from_images)
            .fold(self.layout_height.max(0.0), |acc, v| acc.max(v))
    }
}

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

    #[test]
    fn content_height_conta_a_base_das_imagens() {
        let mut dl = DisplayList::default();
        dl.images.push(ImageItem {
            rect: Rect { x: 0.0, y: 10.0, width: 100.0, height: 150.0 },
            width_px: 200,
            height_px: 150,
            rgba: std::sync::Arc::new(vec![0; 200 * 150 * 4]),
        });
        assert_eq!(dl.content_height(), 160.0);
    }

    #[test]
    fn layout_height_conta_mesmo_sem_item_pintado() {
        let dl = DisplayList { layout_height: 3000.0, ..Default::default() };
        assert_eq!(dl.content_height(), 3000.0);
    }

    #[test]
    fn item_mais_baixo_que_o_layout_ainda_vence() {
        let mut dl = DisplayList { layout_height: 100.0, ..Default::default() };
        dl.boxes.push(BoxItem {
            rect: Rect { x: 0.0, y: 0.0, width: 10.0, height: 500.0 },
            background: None,
            border_color: None,
            border_width: 0.0,
        });
        assert_eq!(dl.content_height(), 500.0);
    }
}
