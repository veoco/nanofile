//! Stable keys for the automatic (people) tag labels.
//!
//! These are always compiled in: the file browser must translate automatic tags
//! that a scan already stored, even while `[face_tags] enabled` is off. The
//! locale key naming (`fb.tag_<label key>`) is mirrored by
//! `server/frontend/core/tag-label.js`.

pub const PERSON_ANY: &str = "person_any";
pub const PERSON_SINGLE: &str = "person_single";
pub const PERSON_GROUP: &str = "person_group";

/// The UI catalog key for an automatic label, or `None` for a key this build
/// does not know (a tag renamed or created by something else stays data).
pub fn locale_key(label_key: &str) -> Option<&'static str> {
    match label_key {
        PERSON_ANY => Some("fb.tag_person_any"),
        PERSON_SINGLE => Some("fb.tag_person_single"),
        PERSON_GROUP => Some("fb.tag_person_group"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::I18n;

    #[test]
    fn every_automatic_label_has_both_translations() {
        for key in [PERSON_ANY, PERSON_SINGLE, PERSON_GROUP] {
            let locale_key = locale_key(key).expect("every automatic label has a catalog key");
            for language in ["en", "zh"] {
                assert!(
                    I18n::get(Some(language)).has(locale_key),
                    "{locale_key} is missing from {language}"
                );
            }
        }
    }

    #[test]
    fn unknown_keys_have_no_catalog_entry() {
        assert_eq!(locale_key("custom"), None);
        assert_eq!(locale_key(""), None);
    }
}
