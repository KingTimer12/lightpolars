//! Emissão de imagem (PNG) a partir de páginas já paginadas.
//!
//! É o par de `pdf-out`: mesma `Page`, mesma `PageGeometry`, mesma ordem de
//! desenho (caixas, imagens, texto). A diferença é que aqui tudo vira pixel,
//! inclusive o texto — num screenshot não há texto selecionável a preservar.
//!
//! Coordenadas: `Page` está em px a 96dpi, origem no canto superior esquerdo da
//! folha, que é a mesma orientação do bitmap. Não há a inversão de eixo que o
//! PDF exige.

use paginate::{Page, PageGeometry};
use render_ir::{FontResource, ImageItem, Rect, TextRun};
use swash::FontRef;
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::zeno::{Format, Vector};
use tiny_skia::{
    FillRule, FilterQuality, Paint, PathBuilder, Pattern, Pixmap, PremultipliedColorU8,
    Rect as SkRect, SpreadMode, Stroke, Transform,
};

/// Teto por lado do PNG gerado, o mesmo critério do rasterizador de SVG: um
/// documento longo com escala alta poderia pedir centenas de MB de bitmap.
pub const LADO_MAXIMO_PX: u32 = 16_384;

/// Fundo do screenshot. O Chromium também parte de branco opaco quando o
/// documento não declara fundo próprio.
const FUNDO: [u8; 4] = [255, 255, 255, 255];

/// Desenha uma página e devolve os bytes de um PNG.
///
/// `escala` é o `deviceScaleFactor`: 1.0 dá 1px de imagem por px CSS, 2.0 dá
/// uma imagem em dobro para telas densas. Devolve `None` só quando a geometria
/// é degenerada (lado zero ou não finito).
pub fn render_png(
    page: &Page,
    fonts: &[FontResource],
    geo: &PageGeometry,
    escala: f32,
) -> Option<Vec<u8>> {
    let pixmap = render_pixmap(page, fonts, geo, escala)?;
    pixmap.encode_png().ok()
}

/// Desenha uma página e devolve os bytes de um JPEG.
///
/// JPEG não tem canal alfa: o desenho já parte de um fundo branco opaco, então
/// basta descartar o alfa (que é 255 em toda a folha).
pub fn render_jpeg(
    page: &Page,
    fonts: &[FontResource],
    geo: &PageGeometry,
    escala: f32,
    qualidade: u8,
) -> Option<Vec<u8>> {
    let pixmap = render_pixmap(page, fonts, geo, escala)?;
    let mut rgb = Vec::with_capacity(pixmap.width() as usize * pixmap.height() as usize * 3);
    for px in pixmap.pixels() {
        let c = px.demultiply();
        rgb.extend_from_slice(&[c.red(), c.green(), c.blue()]);
    }

    let mut saida = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(
        &mut std::io::Cursor::new(&mut saida),
        qualidade.clamp(1, 100),
    )
    .encode(&rgb, pixmap.width(), pixmap.height(), image::ExtendedColorType::Rgb8)
    .ok()?;
    Some(saida)
}

/// Mesmo desenho de `render_png`, exposto sem codificação para os testes
/// poderem inspecionar pixel a pixel sem redecodificar um PNG.
pub fn render_pixmap(
    page: &Page,
    fonts: &[FontResource],
    geo: &PageGeometry,
    escala: f32,
) -> Option<Pixmap> {
    let escala = escala_util(geo, escala)?;
    let largura = (geo.sheet_width * escala).round().max(1.0) as u32;
    let altura = (geo.sheet_height * escala).round().max(1.0) as u32;

    let mut pixmap = Pixmap::new(largura, altura)?;
    pixmap.fill(cor(FUNDO));

    for b in &page.boxes {
        if let Some(c) = b.background {
            preencher_retangulo(&mut pixmap, b.rect, escala, opaca(c));
        }
        if b.border_width > 0.0
            && let Some(c) = b.border_color
        {
            contornar_retangulo(&mut pixmap, b.rect, b.border_width, escala, opaca(c));
        }
    }

    for img in &page.images {
        desenhar_imagem(&mut pixmap, img, escala);
    }

    let mut ctx = ScaleContext::new();
    for run in &page.texts {
        desenhar_run(&mut pixmap, &mut ctx, run, fonts, escala);
    }

    Some(pixmap)
}

/// Escala pedida, reduzida se o bitmap estourar `LADO_MAXIMO_PX`.
fn escala_util(geo: &PageGeometry, escala: f32) -> Option<f32> {
    // Os dois lados precisam ser válidos: checar só o maior deixaria passar uma
    // folha de largura zero.
    for lado in [geo.sheet_width, geo.sheet_height] {
        if !lado.is_finite() || lado <= 0.0 {
            return None;
        }
    }
    if !escala.is_finite() || escala <= 0.0 {
        return None;
    }
    let maior = geo.sheet_width.max(geo.sheet_height);
    Some(escala.min(LADO_MAXIMO_PX as f32 / maior))
}

fn cor(c: [u8; 4]) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(c[0], c[1], c[2], c[3])
}

fn opaca(c: [u8; 3]) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(c[0], c[1], c[2], 255)
}

fn retangulo(r: Rect, escala: f32) -> Option<SkRect> {
    SkRect::from_xywh(
        r.x * escala,
        r.y * escala,
        (r.width * escala).max(f32::EPSILON),
        (r.height * escala).max(f32::EPSILON),
    )
}

fn preencher_retangulo(pixmap: &mut Pixmap, r: Rect, escala: f32, c: tiny_skia::Color) {
    let Some(sk) = retangulo(r, escala) else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color(c);
    paint.anti_alias = true;
    pixmap.fill_rect(sk, &paint, Transform::identity(), None);
}

fn contornar_retangulo(
    pixmap: &mut Pixmap,
    r: Rect,
    largura: f32,
    escala: f32,
    c: tiny_skia::Color,
) {
    let Some(sk) = retangulo(r, escala) else {
        return;
    };
    let Some(caminho) = PathBuilder::from_rect(sk).stroke(
        &Stroke {
            width: (largura * escala).max(0.1),
            ..Default::default()
        },
        1.0,
    ) else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color(c);
    paint.anti_alias = true;
    pixmap.fill_path(
        &caminho,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
}

/// O bitmap da display list é RGBA8 de componentes retos; o `tiny-skia` é
/// pré-multiplicado. Converter aqui é o inverso do que `render-core` faz ao
/// rasterizar SVG.
fn desenhar_imagem(destino: &mut Pixmap, img: &ImageItem, escala: f32) {
    if img.width_px == 0 || img.height_px == 0 {
        return;
    }
    if img.rgba.len() != img.width_px as usize * img.height_px as usize * 4 {
        return;
    }
    let Some(mut origem) = Pixmap::new(img.width_px, img.height_px) else {
        return;
    };
    for (destino_px, fonte) in origem.pixels_mut().iter_mut().zip(img.rgba.chunks_exact(4)) {
        *destino_px = PremultipliedColorU8::from_rgba(
            multiplicar(fonte[0], fonte[3]),
            multiplicar(fonte[1], fonte[3]),
            multiplicar(fonte[2], fonte[3]),
            fonte[3],
        )
        .unwrap_or_else(|| PremultipliedColorU8::from_rgba(0, 0, 0, 0).unwrap());
    }

    let alvo_largura = img.rect.width * escala;
    let alvo_altura = img.rect.height * escala;
    if alvo_largura <= 0.0 || alvo_altura <= 0.0 {
        return;
    }

    // O `Pattern` faz a amostragem; o retângulo recortado é a caixa do layout.
    let padrao = Pattern::new(
        origem.as_ref(),
        SpreadMode::Pad,
        FilterQuality::Bilinear,
        1.0,
        Transform::from_scale(
            alvo_largura / img.width_px as f32,
            alvo_altura / img.height_px as f32,
        )
        .post_translate(img.rect.x * escala, img.rect.y * escala),
    );
    let Some(sk) = SkRect::from_xywh(
        img.rect.x * escala,
        img.rect.y * escala,
        alvo_largura,
        alvo_altura,
    ) else {
        return;
    };
    let paint = Paint {
        shader: padrao,
        anti_alias: true,
        ..Default::default()
    };
    destino.fill_rect(sk, &paint, Transform::identity(), None);
}

fn multiplicar(componente: u8, alfa: u8) -> u8 {
    ((componente as u32 * alfa as u32 + 127) / 255) as u8
}

fn desenhar_run(
    pixmap: &mut Pixmap,
    ctx: &mut ScaleContext,
    run: &TextRun,
    fonts: &[FontResource],
    escala: f32,
) {
    let Some(recurso) = fonts.get(run.font_index) else {
        return;
    };
    let Some(font) = FontRef::from_index(&recurso.bytes, recurso.face_index) else {
        return;
    };

    let tamanho = run.font_size_px * escala;
    if !tamanho.is_finite() || tamanho <= 0.0 {
        return;
    }
    let mut scaler = ctx.builder(font).size(tamanho).hint(false).build();

    for g in &run.glyphs {
        let x = (run.origin_x + g.x) * escala;
        let y = (run.baseline_y + g.y) * escala;
        // A parte inteira posiciona o recorte; a fracionária vai para o
        // rasterizador, senão o texto “dança” meio pixel entre glifos.
        let (xi, yi) = (x.floor(), y.floor());
        let Some(mascara) = Render::new(&[
            Source::ColorOutline(0),
            Source::ColorBitmap(StrikeWith::BestFit),
            Source::Outline,
        ])
        .format(Format::Alpha)
        .offset(Vector::new(x - xi, y - yi))
        .render(&mut scaler, g.id)
        else {
            continue;
        };

        compor_mascara(
            pixmap,
            &mascara.data,
            mascara.placement.width,
            mascara.placement.height,
            xi as i32 + mascara.placement.left,
            yi as i32 - mascara.placement.top,
            run.color,
        );
    }
}

/// Mistura uma máscara de alfa 8 bits sobre o pixmap, na cor dada (source-over).
fn compor_mascara(
    pixmap: &mut Pixmap,
    mascara: &[u8],
    largura: u32,
    altura: u32,
    esquerda: i32,
    topo: i32,
    cor: [u8; 3],
) {
    if largura == 0 || altura == 0 || mascara.len() < (largura * altura) as usize {
        return;
    }
    let (largura_destino, altura_destino) = (pixmap.width() as i32, pixmap.height() as i32);
    let destino = pixmap.pixels_mut();

    for linha in 0..altura as i32 {
        let y = topo + linha;
        if y < 0 || y >= altura_destino {
            continue;
        }
        for coluna in 0..largura as i32 {
            let x = esquerda + coluna;
            if x < 0 || x >= largura_destino {
                continue;
            }
            let a = mascara[(linha as u32 * largura + coluna as u32) as usize];
            if a == 0 {
                continue;
            }
            let idx = (y * largura_destino + x) as usize;
            let antigo = destino[idx];
            let inverso = 255 - a as u32;
            let mistura = |novo: u8, velho: u8| -> u8 {
                ((multiplicar(novo, a) as u32) + (velho as u32 * inverso + 127) / 255).min(255) as u8
            };
            destino[idx] = PremultipliedColorU8::from_rgba(
                mistura(cor[0], antigo.red()),
                mistura(cor[1], antigo.green()),
                mistura(cor[2], antigo.blue()),
                (a as u32 + (antigo.alpha() as u32 * inverso + 127) / 255).min(255) as u8,
            )
            .unwrap_or(antigo);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paginate::{Margins, PageGeometry};
    use render_ir::{BoxItem, Glyph};
    use std::sync::Arc;

    fn geo(w: f32, h: f32) -> PageGeometry {
        PageGeometry {
            sheet_width: w,
            sheet_height: h,
            margins: Margins::default(),
        }
    }

    fn pixel(p: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let px = p.pixel(x, y).unwrap().demultiply();
        [px.red(), px.green(), px.blue(), px.alpha()]
    }

    #[test]
    fn pagina_vazia_sai_branca_no_tamanho_da_folha() {
        let p = render_pixmap(&Page::default(), &[], &geo(100.0, 50.0), 1.0).unwrap();
        assert_eq!((p.width(), p.height()), (100, 50));
        assert_eq!(pixel(&p, 50, 25), [255, 255, 255, 255]);
    }

    #[test]
    fn escala_multiplica_as_dimensoes() {
        let p = render_pixmap(&Page::default(), &[], &geo(100.0, 50.0), 2.0).unwrap();
        assert_eq!((p.width(), p.height()), (200, 100));
    }

    #[test]
    fn caixa_com_fundo_pinta_na_posicao_certa() {
        let page = Page {
            boxes: vec![BoxItem {
                rect: Rect { x: 10.0, y: 10.0, width: 20.0, height: 20.0 },
                background: Some([255, 0, 0]),
                border_color: None,
                border_width: 0.0,
            }],
            ..Default::default()
        };
        let p = render_pixmap(&page, &[], &geo(100.0, 100.0), 1.0).unwrap();
        assert_eq!(pixel(&p, 20, 20), [255, 0, 0, 255], "dentro da caixa");
        assert_eq!(pixel(&p, 5, 5), [255, 255, 255, 255], "fora da caixa");
    }

    #[test]
    fn caixa_escalada_acompanha_o_fator() {
        let page = Page {
            boxes: vec![BoxItem {
                rect: Rect { x: 10.0, y: 10.0, width: 20.0, height: 20.0 },
                background: Some([0, 0, 255]),
                border_color: None,
                border_width: 0.0,
            }],
            ..Default::default()
        };
        let p = render_pixmap(&page, &[], &geo(100.0, 100.0), 2.0).unwrap();
        assert_eq!(pixel(&p, 40, 40), [0, 0, 255, 255], "caixa dobrada");
        assert_eq!(pixel(&p, 10, 10), [255, 255, 255, 255], "antes da caixa dobrada");
    }

    #[test]
    fn imagem_e_esticada_para_a_caixa() {
        // 1x1 verde opaco, desenhado numa caixa de 20x20.
        let page = Page {
            images: vec![ImageItem {
                rect: Rect { x: 10.0, y: 10.0, width: 20.0, height: 20.0 },
                width_px: 1,
                height_px: 1,
                rgba: Arc::new(vec![0, 255, 0, 255]),
            }],
            ..Default::default()
        };
        let p = render_pixmap(&page, &[], &geo(100.0, 100.0), 1.0).unwrap();
        assert_eq!(pixel(&p, 20, 20), [0, 255, 0, 255]);
        assert_eq!(pixel(&p, 5, 5), [255, 255, 255, 255]);
    }

    #[test]
    fn imagem_translucida_mistura_com_o_fundo() {
        // Preto com alfa 50%: sobre branco deve virar cinza médio.
        let page = Page {
            images: vec![ImageItem {
                rect: Rect { x: 0.0, y: 0.0, width: 10.0, height: 10.0 },
                width_px: 1,
                height_px: 1,
                rgba: Arc::new(vec![0, 0, 0, 128]),
            }],
            ..Default::default()
        };
        let p = render_pixmap(&page, &[], &geo(20.0, 20.0), 1.0).unwrap();
        let c = pixel(&p, 5, 5);
        assert!((100..=155).contains(&c[0]), "esperava cinza médio, veio {c:?}");
        assert_eq!(c[3], 255, "o fundo é opaco");
    }

    #[test]
    fn imagem_com_buffer_do_tamanho_errado_e_ignorada() {
        let page = Page {
            images: vec![ImageItem {
                rect: Rect { x: 0.0, y: 0.0, width: 10.0, height: 10.0 },
                width_px: 4,
                height_px: 4,
                rgba: Arc::new(vec![0, 0, 0, 255]),
            }],
            ..Default::default()
        };
        let p = render_pixmap(&page, &[], &geo(20.0, 20.0), 1.0).unwrap();
        assert_eq!(pixel(&p, 5, 5), [255, 255, 255, 255], "não devia pintar nada");
    }

    #[test]
    fn run_com_fonte_inexistente_nao_panica() {
        let page = Page {
            texts: vec![TextRun {
                origin_x: 5.0,
                baseline_y: 20.0,
                font_index: 7,
                font_size_px: 16.0,
                color: [0, 0, 0],
                glyphs: vec![Glyph { id: 1, x: 0.0, y: 0.0 }],
                text: "a".into(),
            }],
            ..Default::default()
        };
        let p = render_pixmap(&page, &[], &geo(50.0, 50.0), 1.0).unwrap();
        assert_eq!(pixel(&p, 10, 15), [255, 255, 255, 255]);
    }

    #[test]
    fn geometria_degenerada_devolve_none() {
        assert!(render_pixmap(&Page::default(), &[], &geo(0.0, 10.0), 1.0).is_none());
        assert!(render_pixmap(&Page::default(), &[], &geo(10.0, 10.0), 0.0).is_none());
        assert!(render_pixmap(&Page::default(), &[], &geo(f32::NAN, 10.0), 1.0).is_none());
    }

    #[test]
    fn escala_absurda_e_limitada_pelo_teto() {
        let p = render_pixmap(&Page::default(), &[], &geo(1000.0, 500.0), 100.0).unwrap();
        assert_eq!(p.width(), LADO_MAXIMO_PX);
    }

    #[test]
    fn png_gerado_tem_assinatura_valida() {
        let bytes = render_png(&Page::default(), &[], &geo(10.0, 10.0), 1.0).unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn jpeg_gerado_tem_assinatura_valida() {
        let bytes = render_jpeg(&Page::default(), &[], &geo(10.0, 10.0), 1.0, 80).unwrap();
        assert_eq!(&bytes[..2], b"\xff\xd8", "SOI de JPEG");
    }

    #[test]
    fn qualidade_fora_da_faixa_nao_panica() {
        assert!(render_jpeg(&Page::default(), &[], &geo(10.0, 10.0), 1.0, 0).is_some());
        assert!(render_jpeg(&Page::default(), &[], &geo(10.0, 10.0), 1.0, 255).is_some());
    }
}
