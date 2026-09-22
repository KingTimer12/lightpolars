//! Where the CPU time of a request goes, phase by phase.
//!
//! The bench measures a request end to end; this splits it, so that optimizing
//! is aimed rather than guessed. Every phase is the real call the handlers
//! make, in the same order, on the same three documents the bench uses.
//!
//!   cargo run --release --example fases -p cdp-server [repetições]

use paginate::PageGeometry;
use render_ir::px_from_mm;
use std::time::{Duration, Instant};

fn main() {
    let repetitions: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(10);

    let documents = [
        ("texto-pesado", text_heavy(60)),
        ("graficos", with_graphics()),
        ("tabelas", tables(40)),
    ];

    println!(
        "{:<14} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9}",
        "documento", "layout", "paginar", "pdf", "pixmap", "png", "total"
    );

    for (name, html) in &documents {
        let mut layout = Duration::ZERO;
        let mut paginating = Duration::ZERO;
        let mut pdf = Duration::ZERO;
        let mut pixmap = Duration::ZERO;
        let mut png = Duration::ZERO;

        for _ in 0..repetitions {
            // The PDF path: layout at the sheet width, slice, encode.
            let geo = pdf_geometry();
            let t = Instant::now();
            let dl = render_core::render_html(html, geo.content_width());
            layout += t.elapsed();

            let t = Instant::now();
            let pages = paginate::paginate(&dl, None, None, &geo);
            paginating += t.elapsed();

            let t = Instant::now();
            let bytes = pdf_out::render_pdf(&pages, &dl.fonts, &geo);
            pdf += t.elapsed();
            std::hint::black_box(bytes);

            // The screenshot path: same layout (cached in the real server), the
            // whole document as one page, drawn and then encoded.
            let t = Instant::now();
            let shot = render_core::render_html(html, 800.0);
            layout += t.elapsed();

            let (page, mut shot_geo) = paginate::whole_document(&shot);
            shot_geo.sheet_width = 800.0;

            let t = Instant::now();
            let drawn = raster_out::render_pixmap(&page, &shot.fonts, &shot_geo, 1.0);
            pixmap += t.elapsed();
            std::hint::black_box(&drawn);

            let t = Instant::now();
            let bytes = raster_out::render_png(&page, &shot.fonts, &shot_geo, 1.0);
            png += t.elapsed();
            std::hint::black_box(bytes);
        }

        // `png` includes drawing the pixmap, since it draws its own; the
        // encoding alone is the difference between the two.
        let encoding = png.saturating_sub(pixmap);
        let total = layout + paginating + pdf + pixmap + encoding;
        let ms = |d: Duration| d.as_secs_f64() * 1000.0 / repetitions as f64;

        println!(
            "{name:<14} {:>9.1} {:>9.1} {:>9.1} {:>9.1} {:>9.1} {:>9.1}",
            ms(layout),
            ms(paginating),
            ms(pdf),
            ms(pixmap),
            ms(encoding),
            ms(total)
        );
    }

    println!(
        "\n(layout conta as duas chamadas — PDF e screenshot — como o servidor faz \
         quando as larguras diferem;\n png é só a codificação, já descontado o pixmap)"
    );
}

/// A4 with the margins Puppeteer defaults to in the bench.
fn pdf_geometry() -> PageGeometry {
    PageGeometry {
        sheet_width: px_from_mm(210.0),
        sheet_height: px_from_mm(297.0),
        margins: paginate::Margins {
            top: px_from_mm(20.0),
            right: px_from_mm(15.0),
            bottom: px_from_mm(20.0),
            left: px_from_mm(15.0),
        },
    }
}

fn text_heavy(paragraphs: usize) -> String {
    let mut html = String::from(
        "<!DOCTYPE html><html><body style='font-family:sans-serif;font-size:12px'>",
    );
    for i in 0..paragraphs {
        html.push_str(&format!(
            "<h2 style='color:#334'>Seção {i}</h2><p>Texto de exemplo com acentuação, \
             números 1234567890 e pontuação — repetido para dar volume ao documento \
             e forçar a paginação a cortar em pontos diferentes.</p>"
        ));
    }
    html.push_str("</body></html>");
    html
}

fn with_graphics() -> String {
    const PNG: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
    let mut html = String::from("<!DOCTYPE html><html><body>");
    for i in 0..20 {
        html.push_str(&format!(
            "<div style='background:#eef;padding:8px;margin:4px'>\
             <svg width='120' height='60' viewBox='0 0 120 60'>\
             <rect x='0' y='0' width='120' height='60' fill='#4a90d9'/>\
             <circle cx='{}' cy='30' r='20' fill='#ffcc00' opacity='0.7'/>\
             <path d='M10 50 L60 10 L110 50 Z' fill='none' stroke='#fff' stroke-width='3'/>\
             </svg>\
             <img src='{PNG}' style='width:80px;height:40px'>\
             <p>Bloco gráfico {i}</p></div>",
            20 + i * 4
        ));
    }
    html.push_str("</body></html>");
    html
}

fn tables(rows: usize) -> String {
    let mut html = String::from(
        "<!DOCTYPE html><html><body><table style='width:100%;border-collapse:collapse'>",
    );
    for r in 0..rows {
        html.push_str("<tr>");
        for c in 0..6 {
            html.push_str(&format!(
                "<td style='border:1px solid #999;padding:4px;background:{}'>{r}.{c}</td>",
                if r % 2 == 0 { "#fff" } else { "#f4f4f4" }
            ));
        }
        html.push_str("</tr>");
    }
    html.push_str("</table></body></html>");
    html
}

