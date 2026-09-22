//! `@font-face` declared inside an inline `<style>`.
//!
//! blitz-dom 0.2.4 only fetches font faces for stylesheets that arrive through
//! the network layer: `CssHandler` (a `<link rel=stylesheet>`) and
//! `StylesheetLoader` (an `@import`) both call `fetch_font_face`, while
//! `process_style_element`, which handles an inline `<style>`, does not. A
//! document whose CSS lives in a `<style>` block therefore parses its
//! `@font-face` rules and never loads them.
//!
//! On a developer machine that failure is invisible: the family falls back to
//! a system font and the text still renders. In a container with no fonts
//! installed there is nothing to fall back to and the text disappears
//! completely. So the faces are collected here and handed to the document as
//! `Resource::Font`.
//!
//! LIMITATION: blitz registers a font under the family name inside the font
//! file, not under the name the `@font-face` rule gives it. A rule that
//! renames a family (`font-family: 'Foo'` over a file whose internal name is
//! `Bar`) will load the bytes but not answer to `Foo`.
//!
//! SECURITY: only `data:` sources are read, same rule as every other resource.

use crate::resource::resolve_data_uri;

/// Font payloads found in the document's inline stylesheets, decompressed and
/// ready to register.
pub fn inline_font_faces(html: &str) -> Vec<Vec<u8>> {
    let mut fonts = Vec::new();
    let mut rest = html;

    while let Some(start) = find_ignoring_case(rest, "@font-face") {
        rest = &rest[start + "@font-face".len()..];
        let Some((block, consumed)) = brace_block(rest) else { break };
        for uri in sources(block) {
            if let Some(bytes) = resolve_data_uri(uri).and_then(|b| decompress(&b)) {
                fonts.push(bytes);
            }
        }
        rest = &rest[consumed..];
    }
    fonts
}

/// The first `{ … }` after the rule name, plus how much of the input to skip
/// to get past it.
fn brace_block(text: &str) -> Option<(&str, usize)> {
    let open = text.find('{')?;
    let close = text[open..].find('}')? + open;
    Some((&text[open..=close], close + 1))
}

/// Every `url(...)` inside the `src` descriptor.
///
/// The whole block is scanned rather than the `src` value alone: a font face
/// has no other `url()`, and matching the descriptor exactly would mean
/// handling comments and nested quotes for no gain.
fn sources(block: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = block;
    while let Some(start) = find_ignoring_case(rest, "url(") {
        rest = &rest[start + 4..];
        let Some(end) = rest.find(')') else { break };
        let uri = rest[..end].trim().trim_matches(['"', '\'']).trim();
        if !uri.is_empty() {
            out.push(uri);
        }
        rest = &rest[end + 1..];
    }
    out
}

/// WOFF and WOFF2 are containers; swash needs the bare sfnt inside. Anything
/// that is already sfnt passes through.
fn decompress(bytes: &[u8]) -> Option<Vec<u8>> {
    match bytes.first_chunk::<4>() {
        Some(b"wOFF") => wuff::decompress_woff1(bytes).ok(),
        Some(b"wOF2") => wuff::decompress_woff2(bytes).ok(),
        // 'OTTO', 0x00010000 and 'true' are sfnt already. Anything else is
        // passed on for the font parser to reject, rather than guessed at.
        _ => Some(bytes.to_vec()),
    }
}

fn find_ignoring_case(haystack: &str, needle: &str) -> Option<usize> {
    let lower = haystack.to_ascii_lowercase();
    lower.find(needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "Oi" as a data URI, standing in for a font file: not sfnt, but the
    /// pass-through branch does not care.
    const FAKE: &str = "data:font/ttf;base64,T2k=";

    #[test]
    fn finds_a_face_in_an_inline_style() {
        let html = format!(
            "<style>@font-face {{ font-family: 'X'; src: url('{FAKE}') format('woff2'); }}</style>"
        );
        assert_eq!(inline_font_faces(&html), vec![b"Oi".to_vec()]);
    }

    #[test]
    fn finds_several_faces() {
        let html = format!(
            "<style>@font-face{{src:url({FAKE})}} @FONT-FACE{{src:url(\"{FAKE}\")}}</style>"
        );
        assert_eq!(inline_font_faces(&html).len(), 2);
    }

    #[test]
    fn ignores_a_face_without_a_data_uri() {
        let html = "<style>@font-face { src: url(https://fonts.example/a.woff2); }</style>";
        assert!(inline_font_faces(html).is_empty(), "a network font must not load");
    }

    #[test]
    fn ignores_urls_outside_a_face() {
        let html = format!("<style>body {{ background: url({FAKE}); }}</style>");
        assert!(inline_font_faces(&html).is_empty());
    }

    #[test]
    fn an_unterminated_block_does_not_loop_forever() {
        let html = format!("<style>@font-face {{ src: url({FAKE})");
        assert!(inline_font_faces(&html).is_empty());
    }

    #[test]
    fn a_document_without_faces_yields_nothing() {
        assert!(inline_font_faces("<p>sem estilo</p>").is_empty());
    }
}
