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
    let bytes = uri.as_bytes();
    if bytes.len() >= 5 && uri[..5].eq_ignore_ascii_case("data:") {
        Some(&uri[5..])
    } else {
        None
    }
}

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
}
