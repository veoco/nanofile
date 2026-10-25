//! Minimal i18n support for the web UI.
//!
//! Translations live in `server/locales/{en,zh}.toml` as flat key/value tables,
//! compressed into the binary with `include-flate`, read through
//! `{{ t.tr("key") }}` / `{{ t.trf("key", &[("name", "value")]) }}`. Missing keys
//! fall back to the key itself, so untranslated strings are easy to spot during
//! development.

use std::collections::HashMap;
use std::sync::LazyLock;

/// Parse a flat locale file into a key/value map.
///
/// `pub(crate)` because the outbound-mail dictionary (`service::mail::i18n`) is a
/// second, deliberately separate set of strings: mail bodies must not be shipped
/// to the browser in every page's `window.__T` blob, and both parse identically.
pub(crate) fn parse_translations(src: &str) -> HashMap<String, String> {
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

include_flate::flate!(static EN_TOML: str from "locales/en.toml" with deflate);
include_flate::flate!(static ZH_TOML: str from "locales/zh.toml" with deflate);

static EN: LazyLock<HashMap<String, String>> = LazyLock::new(|| parse_translations(&EN_TOML));
static ZH: LazyLock<HashMap<String, String>> = LazyLock::new(|| parse_translations(&ZH_TOML));

/// Escape a pre-serialized JSON string for an inline `<script>` block: `<`, `>`,
/// `&` and the U+2028/U+2029 separators, so a value cannot close the element.
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

    /// Normalize a raw language value to a supported tag ("en" / "zh"), or
    /// `None`. Variants such as `zh-CN`, `zh-Hant`, `zh_tw` map to "zh".
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

    /// Resolve the effective language from an Accept-Language header: the first
    /// supported tag, otherwise `default` if supported, otherwise "en".
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

    /// The table for a request's Accept-Language header (anonymous pages such as
    /// login and public share pages).
    pub fn from_headers(headers: &axum::http::HeaderMap, default_lang: &str) -> &'static I18n {
        Self::get(Some(Self::resolve_from_headers(headers, default_lang)))
    }

    /// Translate a key; returns the key itself when missing.
    pub fn tr<'k>(&self, key: &'k str) -> &'k str {
        (**self.dict).get(key).map(|s| s.as_str()).unwrap_or(key)
    }

    /// Whether a key exists, so a caller can render nothing instead of a raw key
    /// (the settings pages do this for each setting's optional help text).
    pub fn has(&self, key: &str) -> bool {
        (**self.dict).contains_key(key)
    }

    /// Translate a key and substitute `{name}` placeholders; `A` is any
    /// `AsRef<str>`, so templates may pass `&x.to_string()` freely.
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

    /// Serialize the whole table as JSON for `window.__T` injection, pre-computed
    /// per language and cached.
    pub fn js_dict(&self) -> &str {
        (**self.js).as_str()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::Path;

    use infra::config::Config;

    use super::*;
    use crate::service::mail::settings::EmailSettings;
    use crate::tasks::spec::{JobKey, ServiceKey};

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

    /// A TOML error empties the whole dictionary (best-effort parse), so pin that the file loaded.
    #[test]
    fn the_source_dictionary_is_not_empty() {
        assert!(
            EN.len() > 100,
            "en.toml parsed into only {} entries — a TOML mistake silently \
             disables every translation",
            EN.len()
        );
    }

    /// Both locales carry the same keys, so a string cannot ship in one language only.
    #[test]
    fn locales_define_the_same_keys() {
        let mut missing_in_zh: Vec<&String> =
            EN.keys().filter(|key| !ZH.contains_key(*key)).collect();
        let mut missing_in_en: Vec<&String> =
            ZH.keys().filter(|key| !EN.contains_key(*key)).collect();
        missing_in_zh.sort();
        missing_in_en.sort();
        assert!(
            missing_in_zh.is_empty(),
            "zh.toml is missing: {missing_in_zh:?}"
        );
        assert!(
            missing_in_en.is_empty(),
            "en.toml is missing: {missing_in_en:?}"
        );
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

    /// Every catalog string must exist in both languages: a fallback would show
    /// an operator a raw id like "setting.server_port_help".
    #[test]
    fn every_catalog_string_is_translated() {
        let mut missing: Vec<String> = Vec::new();
        for def in infra::settings::CATALOG {
            let mut keys = vec![def.label_key(), def.help_key()];
            keys.extend(def.unit_key());
            for key in keys {
                for (lang, table) in [("en", &*EN), ("zh", &*ZH)] {
                    if !table.contains_key(&key) {
                        missing.push(format!("{lang}: {key}"));
                    }
                }
            }
        }
        for def in infra::settings::CATALOG {
            let infra::settings::Kind::Enum(values) = def.kind else {
                continue;
            };
            for value in values {
                let key = def.option_label_key(value);
                for (lang, table) in [("en", &*EN), ("zh", &*ZH)] {
                    if !table.contains_key(&key) {
                        missing.push(format!("{lang}: {key}"));
                    }
                }
            }
        }
        for key in ["setting.list_placeholder", "setting.list_hint"] {
            for (lang, table) in [("en", &*EN), ("zh", &*ZH)] {
                if !table.contains_key(key) {
                    missing.push(format!("{lang}: {key}"));
                }
            }
        }
        for section in infra::settings::Section::ALL {
            for key in [section.title_key(), section.subtitle_key()] {
                for (lang, table) in [("en", &*EN), ("zh", &*ZH)] {
                    if !table.contains_key(key) {
                        missing.push(format!("{lang}: {key}"));
                    }
                }
            }
        }
        for group in infra::settings::GROUPS {
            for (lang, table) in [("en", &*EN), ("zh", &*ZH)] {
                if !table.contains_key(group.title_key) {
                    missing.push(format!("{lang}: {}", group.title_key));
                }
            }
        }
        assert!(missing.is_empty(), "untranslated: {missing:?}");
    }

    /// The converse of `every_catalog_string_is_translated`: a key whose reader
    /// moved away stays forever. `admin.*` is where a key is built at run time;
    /// `server/tests` is not scanned, or a dead key outlives it in its own test.
    #[test]
    fn no_admin_key_is_unasked_for() {
        let mut covered: HashSet<String> = generated_admin_keys().into_iter().collect();
        let mut tree = String::new();
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        for dir in ["src", "templates", "frontend"] {
            collect_text(&root.join(dir), &mut tree);
        }
        covered.extend(admin_keys_in(&tree).into_iter().map(str::to_string));

        let mut unused: Vec<&str> = EN
            .keys()
            .filter(|key| key.starts_with("admin.") && !covered.contains(key.as_str()))
            .map(String::as_str)
            .collect();
        unused.sort_unstable();
        unused.dedup();
        assert!(
            unused.is_empty(),
            "no template or module asks for these keys: {unused:#?} — delete \
             them, or add the generator that builds them"
        );
    }

    /// Every whole `admin.<key>` string in `text`.
    ///
    /// Whole rather than by substring (the paused-mail key prefixes its `_on`
    /// sibling), and not glued to a preceding identifier (`form.is_admin`). A key
    /// written in a comment is a reference like any other, hence none here.
    fn admin_keys_in(text: &str) -> Vec<&str> {
        const MARKER: &str = "admin.";
        let mut out = Vec::new();
        let mut rest = text;
        while let Some(at) = rest.find(MARKER) {
            let glued = rest[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
            let after = &rest[at + MARKER.len()..];
            let name_len = after
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(after.len());
            if !glued && name_len > 0 {
                out.push(&rest[at..at + MARKER.len() + name_len]);
            }
            rest = after;
        }
        out
    }

    /// The `admin.*` keys no file writes whole: the slug is only known at run time.
    fn generated_admin_keys() -> Vec<String> {
        let mut keys = Vec::new();
        let mut slug_keys = |prefix: &str, slug: &str| {
            let slug = slug.replace('-', "_");
            keys.push(format!("{prefix}{slug}"));
            keys.push(format!("{prefix}{slug}_desc"));
        };
        for key in JobKey::ALL {
            slug_keys("admin.job_", key.as_str());
        }
        for key in ServiceKey::ALL {
            slug_keys("admin.service_", key.as_str());
        }
        // A retired job keeps its name only: the journal still holds rows under
        // that slug, so the label has a reader, but no registry row to describe.
        for (_, name_key) in crate::tasks::spec::RETIRED_JOBS {
            keys.push((*name_key).to_string());
        }

        // Not a hard-coded list: `missing()` is what `missing_labels` translates,
        // so the mail fields are elicited by breaking all three at once.
        let mut config = Config::default();
        config.email.host = String::new();
        config.email.port = 0;
        config.email.from_address = "not-an-address".to_string();
        for field in EmailSettings::from_config(&config).missing() {
            keys.push(format!("admin.email_field_{field}"));
        }
        keys
    }

    /// Append every readable file under `dir` to `out`, so one pass can search a
    /// whole tree; a path that will not read as UTF-8 is skipped.
    fn collect_text(dir: &Path, out: &mut String) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_text(&path, out);
            } else if let Ok(text) = std::fs::read_to_string(&path) {
                out.push_str(&text);
                out.push('\n');
            }
        }
    }

    /// A numeric setting's unit renders beside the field, so the label must not
    /// repeat it ("Max upload size (MB) MB").
    #[test]
    fn a_unit_is_not_repeated_in_the_label() {
        for def in infra::settings::CATALOG {
            let Some(unit_key) = def.unit_key() else {
                continue;
            };
            for table in [&*EN, &*ZH] {
                let unit = table.get(&unit_key).expect("the unit is translated");
                let label = table.get(&def.label_key()).expect("the label exists");
                assert!(
                    !label.contains(&format!("({unit})"))
                        && !label.contains(&format!("（{unit}）")),
                    "{} repeats its unit in the label: {label:?}",
                    def.key
                );
            }
        }
    }

    /// Every tray string must exist in both languages, or the menu shows raw keys
    /// like "tray.quit".
    #[test]
    fn tray_keys_present_in_all_languages() {
        const KEYS: &[&str] = &[
            "tray.open_web",
            "tray.launch_at_login",
            "tray.service_autostart",
            "tray.open_config",
            "tray.quit",
            "tray.tooltip",
            "tray.notify_title",
            "tray.notify_body",
            "tray.client_mode_body",
            "tray.client_mode_tooltip",
            "tray.elevated_user_fallback",
            "tray.elevated_body",
            "tray.service_confirm_enable",
            "tray.service_confirm_replace",
            "tray.service_confirm_replace_missing",
            "tray.service_confirm_disable",
            "tray.service_confirm_disable_running",
            "tray.service_installed",
            "tray.service_removed",
            "tray.service_failed",
            "tray.service_launch_failed",
            "tray.service_preflight_failed",
            "tray.service_advisories",
            "tray.autostart_superseded_by_service",
            "tray.quit_service_stopped",
            "tray.quit_service_running",
            "tray.quit_other_service",
        ];
        for lang in [None, Some("en"), Some("zh")] {
            let t = I18n::get(lang);
            for key in KEYS {
                assert_ne!(t.tr(key), *key, "missing {key} for {}", t.lang);
            }
        }
        assert_eq!(I18n::get(Some("zh-CN")).tr("tray.quit"), "退出");
        assert_eq!(I18n::get(Some("en")).tr("tray.quit"), "Quit");
        // The elevated-account dialog substitutes the username.
        assert!(
            I18n::get(Some("zh"))
                .trf("tray.elevated_body", &[("user", "admin@x.com")])
                .contains("admin@x.com")
        );
    }
}
