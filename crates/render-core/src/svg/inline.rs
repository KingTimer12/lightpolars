//! Rewrites `<svg>` written directly in the HTML into an `<img>`.
//!
//! blitz-dom 0.2.4 only turns SVG into `ImageData::Svg` when it arrives as a
//! *resource* (`<img src=...>` or `background-image`). An `<svg>` written
//! inline becomes a zero-sized replaced element with no data attached. Turning
//! each `<svg>…</svg>` into `<img src="data:image/svg+xml;base64,…">` before
//! parsing lets the existing image path handle parsing, intrinsic size and
//! layout.
//!
//! SECURITY RULE: the rewrite only ever produces `data:`, and the content comes
//! from the HTML itself — no socket or file is opened here.

use base64::Engine as _;

/// Attributes of the `<svg>` that still make sense on the `<img>` replacing it.
/// The rest (`viewBox`, `xmlns`, `fill`…) belong to the SVG and travel inside
/// the data: URI.
const FORWARDED_ATTRS: [&str; 5] = ["id", "class", "style", "width", "height"];

/// Replaces every `<svg>…</svg>` in the HTML with an `<img src="data:…">`.
pub fn inline_svg_to_img(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;

    while let Some(start) = find_svg_open(rest) {
        out.push_str(&rest[..start]);
        let Some(len) = svg_block_len(&rest[start..]) else {
            // `<svg` with no closing tag: copy the remainder verbatim rather
            // than dropping content.
            rest = &rest[start..];
            break;
        };
        let block = &rest[start..start + len];
        out.push_str(&block_to_img(block));
        rest = &rest[start + len..];
    }

    out.push_str(rest);
    out
}

/// Index of the next `<svg` that opens a real element, not a longer name like
/// `<svgfoo`.
fn find_svg_open(text: &str) -> Option<usize> {
    let lowercase = text.to_ascii_lowercase();
    let bytes = lowercase.as_bytes();
    let mut from = 0;
    while let Some(pos) = lowercase[from..].find("<svg") {
        let abs = from + pos;
        if opens_svg_at(bytes, abs) {
            return Some(abs);
        }
        from = abs + 4;
    }
    None
}

/// True if `<svg` starts an opening tag at `i` (in already-lowercased text).
fn opens_svg_at(bytes: &[u8], i: usize) -> bool {
    if !bytes[i..].starts_with(b"<svg") {
        return false;
    }
    matches!(
        bytes.get(i + 4),
        Some(b'>') | Some(b'/') | Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r')
    )
}

/// Length of the `<svg>…</svg>` block starting at the front of `text`, counting
/// nesting (SVG allows `<svg>` inside `<svg>`).
fn svg_block_len(text: &str) -> Option<usize> {
    let lowercase = text.to_ascii_lowercase();
    let bytes = lowercase.as_bytes();
    let mut depth = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        if lowercase[i..].starts_with("</svg") {
            let end = i + lowercase[i..].find('>')? + 1;
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(end);
            }
            i = end;
        } else if opens_svg_at(bytes, i) {
            let end = i + lowercase[i..].find('>')? + 1;
            // A self-closing tag (`<svg ... />`) opens no level.
            if !lowercase[i..end].ends_with("/>") {
                depth += 1;
            } else if depth == 0 {
                return Some(end);
            }
            i = end;
        } else {
            i += 1;
        }
    }
    None
}

fn block_to_img(block: &str) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(block.as_bytes());
    let mut img = String::from("<img");
    for (name, value) in tag_attributes(block) {
        if FORWARDED_ATTRS.contains(&name.to_ascii_lowercase().as_str()) {
            img.push_str(&format!(" {name}=\"{}\"", escape_quotes(&value)));
        }
    }
    img.push_str(&format!(" src=\"data:image/svg+xml;base64,{b64}\" />"));
    img
}

/// `name="value"` pairs from the opening tag. Minimal parser: accepts single
/// quotes, double quotes and unquoted values, which is what real report HTML
/// produces.
fn tag_attributes(block: &str) -> Vec<(String, String)> {
    let end = block.find('>').unwrap_or(block.len());
    let body = &block[4..end]; // skip "<svg"
    let chars: Vec<char> = body.chars().collect();
    let mut pairs = Vec::new();
    let mut i = 0;

    while i < chars.len() {
        while i < chars.len() && (chars[i].is_whitespace() || chars[i] == '/') {
            i += 1;
        }
        let name_start = i;
        while i < chars.len() && !chars[i].is_whitespace() && chars[i] != '=' && chars[i] != '/' {
            i += 1;
        }
        if i == name_start {
            break;
        }
        let name: String = chars[name_start..i].iter().collect();

        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= chars.len() || chars[i] != '=' {
            pairs.push((name, String::new()));
            continue;
        }
        i += 1; // '='
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }

        let value = if i < chars.len() && (chars[i] == '"' || chars[i] == '\'') {
            let quote = chars[i];
            i += 1;
            let start = i;
            while i < chars.len() && chars[i] != quote {
                i += 1;
            }
            let v: String = chars[start..i].iter().collect();
            i += 1; // closing quote
            v
        } else {
            let start = i;
            while i < chars.len() && !chars[i].is_whitespace() {
                i += 1;
            }
            chars[start..i].iter().collect()
        };
        pairs.push((name, value));
    }

    pairs
}

fn escape_quotes(value: &str) -> String {
    value.replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SVG: &str = r##"<svg width="20" height="10" xmlns="http://www.w3.org/2000/svg"><rect width="20" height="10" fill="#f00"/></svg>"##;

    #[test]
    fn inline_svg_becomes_an_img_with_a_data_uri() {
        let out = inline_svg_to_img(&format!("<p>before</p>{SVG}<p>after</p>"));
        assert!(out.starts_with("<p>before</p><img "), "{out}");
        assert!(out.ends_with("<p>after</p>"), "{out}");
        assert!(out.contains("src=\"data:image/svg+xml;base64,"), "{out}");
    }

    #[test]
    fn presentation_attributes_move_to_the_img() {
        let input = r##"<svg class="logo" style="height:150px" width="20" height="10" fill="#f00"></svg>"##;
        let out = inline_svg_to_img(input);
        assert!(out.contains(r#"class="logo""#), "{out}");
        assert!(out.contains(r#"style="height:150px""#), "{out}");
        assert!(out.contains(r#"width="20""#), "{out}");
        assert!(!out.contains("fill="), "fill belongs to the SVG, not the img: {out}");
    }

    #[test]
    fn nested_svg_closes_at_the_right_level() {
        let input = r#"<svg width="10" height="10"><svg width="5" height="5"></svg></svg>END"#;
        let out = inline_svg_to_img(input);
        assert!(out.ends_with("END"), "{out}");
        assert_eq!(out.matches("<img").count(), 1, "nesting produced two imgs: {out}");
    }

    #[test]
    fn a_self_closing_svg_becomes_one_img() {
        let out = inline_svg_to_img(r#"<svg width="10" height="10"/>END"#);
        assert_eq!(out.matches("<img").count(), 1, "{out}");
        assert!(out.ends_with("END"), "{out}");
    }

    #[test]
    fn text_without_svg_passes_through_untouched() {
        let input = "<p>svgnot</p><svgfoo>x</svgfoo>";
        assert_eq!(inline_svg_to_img(input), input);
    }

    #[test]
    fn unclosed_svg_does_not_lose_content() {
        let input = "<p>a</p><svg width=\"10\"><rect/>";
        assert_eq!(inline_svg_to_img(input), input);
    }
}
