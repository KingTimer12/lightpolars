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
