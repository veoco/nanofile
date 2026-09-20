//! Translations for outbound mail.
//!
//! Deliberately a second dictionary rather than more keys in
//! [`crate::i18n::I18n`]: that one is serialized into `window.__T` on every
//! page, and mail bodies are long, HTML-bearing and useless to the browser.
//!
//! The recipient's stored language (`users.language`) picks the table; an
//! unset or unsupported value falls back to the configured default UI
//! language, and then to English — the same resolution the Web UI uses.

use std::collections::HashMap;
use std::sync::LazyLock;

static EMAIL_EN: LazyLock<HashMap<String, String>> = LazyLock::new(|| {
    crate::i18n::parse_translations(include_str!("../../../locales/email_en.toml"))
});
static EMAIL_ZH: LazyLock<HashMap<String, String>> = LazyLock::new(|| {
    crate::i18n::parse_translations(include_str!("../../../locales/email_zh.toml"))
});

static MAIL_EN: MailStrings = MailStrings {
    lang: "en",
    dict: &EMAIL_EN,
};
static MAIL_ZH: MailStrings = MailStrings {
    lang: "zh",
    dict: &EMAIL_ZH,
};

/// One language's mail strings.
pub struct MailStrings {
    /// `en` or `zh`, for the `Content-Language`-style bookkeeping and tests.
    pub lang: &'static str,
    dict: &'static LazyLock<HashMap<String, String>>,
}

impl MailStrings {
    /// The table for a language tag, falling back to the site default and then
    /// to English. An unsupported tag must never leave a message untranslated
    /// in a way the reader cannot act on, so there is no "unknown" table.
    pub fn get(lang: Option<&str>, default_lang: &str) -> &'static MailStrings {
        let resolved = lang
            .and_then(crate::i18n::I18n::normalize_lang)
            .or_else(|| crate::i18n::I18n::normalize_lang(default_lang));
        match resolved {
            Some("zh") => &MAIL_ZH,
            _ => &MAIL_EN,
        }
    }

    /// Translate a key; a missing key returns the key itself, like the page
    /// dictionary does, so an untranslated string is obvious instead of blank.
    pub fn tr<'k>(&self, key: &'k str) -> &'k str {
        (**self.dict).get(key).map(String::as_str).unwrap_or(key)
    }

    /// Translate and substitute `{name}` placeholders.
    pub fn trf<'k>(&self, key: &'k str, args: &[(&'k str, String)]) -> String {
        let mut out = self.tr(key).to_string();
        for (name, value) in args {
            out = out.replace(&format!("{{{name}}}"), value);
        }
        out
    }

    /// Every key defined for this language. Used by the locale-parity test.
    pub fn keys(&self) -> Vec<&String> {
        let mut keys: Vec<&String> = (**self.dict).keys().collect();
        keys.sort();
        keys
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both mail dictionaries must define the same keys, for the same reason the
    /// page locales do: a key that ships in one language only renders as its own
    /// identifier in the other.
    #[test]
    fn mail_locales_define_the_same_keys() {
        let en = MailStrings::get(Some("en"), "en");
        let zh = MailStrings::get(Some("zh"), "zh");
        let missing_in_zh: Vec<&String> = en
            .keys()
            .into_iter()
            .filter(|key| !zh.keys().contains(key))
            .collect();
        let missing_in_en: Vec<&String> = zh
            .keys()
            .into_iter()
            .filter(|key| !en.keys().contains(key))
            .collect();
        assert!(
            missing_in_zh.is_empty(),
            "email_zh.toml is missing: {missing_in_zh:?}"
        );
        assert!(
            missing_in_en.is_empty(),
            "email_en.toml is missing: {missing_in_en:?}"
        );
    }

    #[test]
    fn the_source_dictionary_is_not_empty() {
        // 5 kinds x (subject, html, text) + brand, footer and the two layout
        // parts; a lower count means a TOML mistake emptied part of the file.
        assert!(
            EMAIL_EN.len() >= 19,
            "email_en.toml parsed into only {} entries — a TOML mistake would \
             silently disable every mail translation",
            EMAIL_EN.len()
        );
    }

    #[test]
    fn language_falls_back_through_default_to_english() {
        assert_eq!(MailStrings::get(Some("zh-CN"), "en").lang, "zh");
        assert_eq!(MailStrings::get(Some("zh_tw"), "en").lang, "zh");
        assert_eq!(MailStrings::get(Some("en"), "zh").lang, "en");
        // An unset or unsupported preference takes the site default...
        assert_eq!(MailStrings::get(None, "zh").lang, "zh");
        assert_eq!(MailStrings::get(Some("fr"), "zh").lang, "zh");
        // ...and an unsupported default lands on English.
        assert_eq!(MailStrings::get(None, "fr").lang, "en");
    }

    #[test]
    fn missing_keys_render_as_the_key() {
        assert_eq!(MailStrings::get(None, "en").tr("email.nope"), "email.nope");
    }
}
