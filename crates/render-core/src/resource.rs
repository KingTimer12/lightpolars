//! Resolution of resources referenced by the HTML.
//!
//! SECURITY RULE: only `data:` is accepted. The HTML this engine processes is
//! untrusted (in helpers/_pdf.js it is AI-generated). Any other scheme resolves
//! as absent, and no function in this module opens a socket or a file.

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
    if bytes.len() >= 5 && bytes[..5].eq_ignore_ascii_case(b"data:") {
        Some(&uri[5..])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_base64_data_uri() {
        // "Oi" in base64 is "T2k="
        let out = resolve_data_uri("data:text/plain;base64,T2k=").unwrap();
        assert_eq!(out, b"Oi");
    }

    #[test]
    fn decodes_data_uri_without_base64() {
        let out = resolve_data_uri("data:text/plain,Oi").unwrap();
        assert_eq!(out, b"Oi");
    }

    #[test]
    fn rejects_http_https_and_file() {
        assert!(resolve_data_uri("http://exemplo.com/a.png").is_none());
        assert!(resolve_data_uri("https://exemplo.com/a.png").is_none());
        assert!(resolve_data_uri("file:///etc/passwd").is_none());
        assert!(resolve_data_uri("//exemplo.com/a.png").is_none());
        assert!(resolve_data_uri("/var/secret").is_none());
    }

    #[test]
    fn uppercase_does_not_disguise_another_scheme() {
        // Must not be mistaken for data:
        assert!(resolve_data_uri("javascript:alert(1)").is_none());
        // A legitimate data: in upper case stays valid
        assert!(resolve_data_uri("DATA:text/plain,Oi").is_some());
    }

    #[test]
    fn invalid_base64_returns_none_instead_of_panicking() {
        assert!(resolve_data_uri("data:image/png;base64,!!!not-base64!!!").is_none());
    }

    #[test]
    fn multibyte_input_does_not_panic() {
        // "é" takes bytes 4 and 5, so cutting at 5 would land mid-character.
        assert!(resolve_data_uri("aaaaé").is_none());
        assert!(resolve_data_uri("é").is_none());
        assert!(resolve_data_uri("dataé").is_none());
        // A legitimate data: followed by multibyte content keeps working
        assert_eq!(resolve_data_uri("data:text/plain,ação").unwrap(), "ação".as_bytes());
    }
}
