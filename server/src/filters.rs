//! Askama template filters. The module path `filters::` is what the Askama
//! derive resolves custom filters to, so these must live at crate root.

use std::fmt::Display;

/// Serialize `input` as a JSON string literal that is safe inside an inline
/// `<script>` block: unlike the built-in `json` filter it also escapes `<`, `>`,
/// `&` and U+2028/U+2029, so a value containing `</script>` cannot break out of
/// the surrounding script element.
fn js_json_escape(input: &str) -> String {
    serde_json::to_string(input)
        .unwrap_or_else(|_| "\"\"".to_string())
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

/// Askama filter — JS-safe JSON string embedding (see [`js_json_escape`]).
#[askama::filter_fn]
pub fn js_json(value: &dyn Display, _: &dyn askama::Values) -> askama::Result<String> {
    Ok(js_json_escape(&value.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_script_closing_tag() {
        let out = js_json_escape("</script><script>alert(1)</script>");
        assert!(
            !out.contains("</script>"),
            "script breakout possible: {out}"
        );
        assert!(out.contains("\\u003c/script"));
    }

    #[test]
    fn keeps_quotes_escaped() {
        assert_eq!(js_json_escape("he said \"hi\""), r#""he said \"hi\"""#);
    }

    #[test]
    fn plain_string_roundtrips() {
        assert_eq!(js_json_escape("plain"), "\"plain\"");
    }
}
