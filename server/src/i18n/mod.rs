//! Minimal i18n support for the web UI.
//!
//! Translations live in `server/locales/{en,zh}.toml` as flat key/value tables,
//! compiled in at build time via `include_str!`. Each page template carries a
//! `t: &'static I18n` field and renders strings through
//! `{{ t.tr("key") }}` / `{{ t.trf("key", &[("name", "value")]) }}`.
//!
//! Missing keys fall back to the key itself so untranslated strings are easy to
//! spot during development.

use std::collections::HashMap;
use std::sync::LazyLock;

fn parse_translations(src: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    if let Ok(value) = toml::from_str::<toml::Value>(src) {
        flatten_toml(&value, "", &mut map);
    }
    map
}

/// Recursively flatten a TOML table into dotted keys (`nav.libraries`).
fn flatten_toml(value: &toml::Value, prefix: &str, out: &mut HashMap<String, String>) {
    match value {
        toml::Value::Table(table) => {
            for (k, v) in table {
                let key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten_toml(v, &key, out);
            }
        }
        toml::Value::String(s) => {
            out.insert(prefix.to_string(), s.clone());
        }
        _ => {}
    }
}

static EN: LazyLock<HashMap<String, String>> =
    LazyLock::new(|| parse_translations(include_str!("../../locales/en.toml")));
static ZH: LazyLock<HashMap<String, String>> =
    LazyLock::new(|| parse_translations(include_str!("../../locales/zh.toml")));

/// Escape a pre-serialized JSON string so it is safe to embed inside an inline
/// `<script>` block: `<`, `>`, `&` and the U+2028/U+2029 line separators are
/// escaped so a value cannot break out of the surrounding script element.
fn escape_inline_script(json: &str) -> String {
    json.replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

/// Pre-serialized JSON for `window.__T`, computed once per language.
static EN_JSON: LazyLock<String> = LazyLock::new(|| {
    let json = serde_json::to_string(&*EN).unwrap_or_else(|_| "{}".to_string());
    escape_inline_script(&json)
});
static ZH_JSON: LazyLock<String> = LazyLock::new(|| {
    let json = serde_json::to_string(&*ZH).unwrap_or_else(|_| "{}".to_string());
    escape_inline_script(&json)
});

static I18N_EN: I18n = I18n {
    lang: "en",
    dict: &EN,
    js: &EN_JSON,
};
static I18N_ZH: I18n = I18n {
    lang: "zh",
    dict: &ZH,
    js: &ZH_JSON,
};

/// A translation table for one language.
pub struct I18n {
    /// BCP-47-ish tag used in `<html lang>`.
    pub lang: &'static str,
    dict: &'static LazyLock<HashMap<String, String>>,
    js: &'static LazyLock<String>,
}

impl I18n {
    /// Look up the translation table for a stored user preference.
    pub fn get(lang: Option<&str>) -> &'static I18n {
        match lang {
            Some(l)
                if l.trim()
                    .get(..2)
                    .is_some_and(|p| p.eq_ignore_ascii_case("zh")) =>
            {
                &I18N_ZH
            }
            _ => &I18N_EN,
        }
    }

    /// Normalize a raw language value to a supported tag ("en" / "zh"),
    /// or `None` for unsupported languages. Accepts variants such as
    /// `zh-CN`, `zh-Hant`, `zh_tw` (all map to "zh").
    pub fn normalize_lang(lang: &str) -> Option<&'static str> {
        let trimmed = lang.trim();
        if trimmed.eq_ignore_ascii_case("en") {
            Some("en")
        } else if trimmed
            .get(..2)
            .is_some_and(|p| p.eq_ignore_ascii_case("zh"))
        {
            Some("zh")
        } else {
            None
        }
    }

    /// Resolve the effective language from an Accept-Language header.
    ///
    /// Returns the first supported language tag, otherwise `default` if it is a
    /// supported language, otherwise "en".
    pub fn resolve(accept_language: Option<&str>, default: &str) -> &'static str {
        if let Some(header) = accept_language {
            for part in header.split(',') {
                let tag = part.trim().split(';').next().unwrap_or("").trim();
                let base = tag
                    .split(['-', '_'])
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase();
                match base.as_str() {
                    "zh" => return "zh",
                    "en" => return "en",
                    _ => {}
                }
            }
        }
        if default.eq_ignore_ascii_case("zh") {
            "zh"
        } else {
            "en"
        }
    }

    /// Resolve the UI language from a request's Accept-Language header.
    pub fn resolve_from_headers(
        headers: &axum::http::HeaderMap,
        default_lang: &str,
    ) -> &'static str {
        let accept = headers
            .get(axum::http::header::ACCEPT_LANGUAGE)
            .and_then(|v| v.to_str().ok());
        Self::resolve(accept, default_lang)
    }

    /// Select the translation table from a request's Accept-Language header
    /// (used by anonymous pages such as login and public share pages).
    pub fn from_headers(headers: &axum::http::HeaderMap, default_lang: &str) -> &'static I18n {
        Self::get(Some(Self::resolve_from_headers(headers, default_lang)))
    }

    /// Translate a key; returns the key itself when missing.
    pub fn tr<'k>(&self, key: &'k str) -> &'k str {
        (**self.dict).get(key).map(|s| s.as_str()).unwrap_or(key)
    }

    /// Translate a key and substitute `{name}` placeholders.
    ///
    /// `A` is any `AsRef<str>` so callers may pass `&str`, `&String` or
    /// `String` values freely (templates frequently pass `&x.to_string()`).
    pub fn trf<'k, A>(&self, key: &'k str, args: &[(&'k str, A)]) -> String
    where
        A: AsRef<str>,
    {
        let mut out = self.tr(key).to_string();
        for (name, value) in args {
            out = out.replace(&format!("{{{name}}}"), value.as_ref());
        }
        out
    }

    /// Serialize the whole table as JSON for `window.__T` injection.
    /// The result is pre-computed per language and cached.
    pub fn js_dict(&self) -> &str {
        (**self.js).as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tr_returns_translation_and_falls_back_to_key() {
        let en = I18n::get(None);
        assert_eq!(en.lang, "en");
        assert_eq!(en.tr("app.name"), "Nanofile");
        assert_eq!(en.tr("no.such.key"), "no.such.key");

        let zh = I18n::get(Some("zh"));
        assert_eq!(zh.lang, "zh");
        assert_eq!(zh.tr("app.name"), "Nanofile");
    }

    #[test]
    fn trf_substitutes_placeholders() {
        let en = I18n::get(None);
        assert_eq!(en.trf("test.hello", &[("name", "World")]), "Hello, World!");
    }

    #[test]
    fn get_matches_zh_variants() {
        assert_eq!(I18n::get(Some("zh")).lang, "zh");
        assert_eq!(I18n::get(Some("zh-CN")).lang, "zh");
        assert_eq!(I18n::get(Some("ZH")).lang, "zh");
        assert_eq!(I18n::get(Some("en")).lang, "en");
        assert_eq!(I18n::get(Some("de")).lang, "en");
        assert_eq!(I18n::get(None).lang, "en");
    }

    #[test]
    fn resolve_parses_accept_language() {
        assert_eq!(I18n::resolve(Some("zh-CN,zh;q=0.9"), "en"), "zh");
        assert_eq!(I18n::resolve(Some("en-US,en;q=0.8"), "en"), "en");
        assert_eq!(I18n::resolve(Some("fr-FR"), "en"), "en");
        assert_eq!(I18n::resolve(Some("fr-FR"), "zh"), "zh");
        assert_eq!(I18n::resolve(None, "zh"), "zh");
        assert_eq!(I18n::resolve(None, "en"), "en");
    }

    #[test]
    fn js_dict_is_valid_json_object() {
        let zh = I18n::get(Some("zh"));
        let parsed: HashMap<String, String> = serde_json::from_str(zh.js_dict()).unwrap();
        assert!(parsed.contains_key("app.name"));
        assert!(parsed.contains_key("ui.create"), "ui.create missing");
        assert_eq!(parsed.get("ui.create").unwrap(), "创建");
    }

    #[test]
    fn escape_inline_script_prevents_script_breakout() {
        let out = escape_inline_script(r#"{"k":"</script><script>alert(1)</script>"}"#);
        assert!(
            !out.contains("</script>"),
            "script breakout possible: {out}"
        );
        assert!(out.contains("\\u003c/script"));
    }
}
