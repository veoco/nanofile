//! The three automatic tags: their stable keys, colors and search aliases.
//!
//! The display text lives in the UI catalog (`server/locales/`), keyed by
//! `domain::tag::locale_key`, so the scanner and the file browser cannot drift
//! apart on a tag's name.

use crate::i18n::I18n;

pub use crate::domain::tag::{PERSON_ANY, PERSON_GROUP, PERSON_SINGLE};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LabelDef {
    pub key: &'static str,
    pub color: &'static str,
}

pub const ALL: &[LabelDef] = &[
    LabelDef {
        key: PERSON_ANY,
        color: "#3b82f6",
    },
    LabelDef {
        key: PERSON_SINGLE,
        color: "#06b6d4",
    },
    LabelDef {
        key: PERSON_GROUP,
        color: "#8b5cf6",
    },
];

/// Labels implied by a qualified face count.
pub fn for_count(face_count: usize) -> Vec<&'static LabelDef> {
    let mut labels = Vec::new();
    if face_count >= 1 {
        labels.push(&ALL[0]);
    }
    if face_count == 1 {
        labels.push(&ALL[1]);
    } else if face_count >= 2 {
        labels.push(&ALL[2]);
    }
    labels
}

pub fn by_key(key: &str) -> Option<&'static LabelDef> {
    ALL.iter().find(|label| label.key == key)
}

/// The scanner's stored tag name in the given language. Display re-translates
/// by `label_key`, so this is only the name a shared repository stores; search
/// aliases cover both languages.
pub fn display_name(label: &LabelDef, language: &str) -> &'static str {
    let key =
        crate::domain::tag::locale_key(label.key).expect("every automatic label has a catalog key");
    I18n::get(Some(language)).tr(key)
}

/// Query aliases used by the search phase. Matching is case-insensitive and
/// exact after trimming; a tag's own name is also matched by SQL.
pub fn keys_for_query(query: &str) -> Vec<&'static str> {
    let q = query.trim().to_ascii_lowercase();
    let mut keys = Vec::new();
    for label in ALL {
        let matches = match label.key {
            PERSON_ANY => matches!(
                q.as_str(),
                "人物"
                    | "人脸"
                    | "有人"
                    | "有人物"
                    | "人物照"
                    | "people"
                    | "person"
                    | "face"
                    | "faces"
            ),
            PERSON_SINGLE => matches!(
                q.as_str(),
                "单人" | "一个人" | "独照" | "单人照" | "single" | "solo" | "one person"
            ),
            PERSON_GROUP => matches!(
                q.as_str(),
                "合影" | "合照" | "多人" | "多人照" | "group" | "group photo" | "multiple people"
            ),
            _ => false,
        };
        if matches {
            keys.push(label.key);
        }
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_map_to_tags() {
        assert!(for_count(0).is_empty());
        assert_eq!(
            for_count(1).iter().map(|l| l.key).collect::<Vec<_>>(),
            vec![PERSON_ANY, PERSON_SINGLE]
        );
        assert_eq!(
            for_count(2).iter().map(|l| l.key).collect::<Vec<_>>(),
            vec![PERSON_ANY, PERSON_GROUP]
        );
    }

    #[test]
    fn chinese_and_english_aliases_resolve() {
        assert_eq!(keys_for_query("人物"), vec![PERSON_ANY]);
        assert_eq!(keys_for_query("people"), vec![PERSON_ANY]);
        assert_eq!(keys_for_query("合影"), vec![PERSON_GROUP]);
        assert_eq!(keys_for_query("group photo"), vec![PERSON_GROUP]);
    }

    #[test]
    fn display_names_follow_the_ui_catalog() {
        assert_eq!(display_name(&ALL[0], "zh"), "人物");
        assert_eq!(display_name(&ALL[0], "zh-CN"), "人物");
        assert_eq!(display_name(&ALL[0], "en"), "People");
        assert_eq!(display_name(&ALL[0], "de"), "People");
        assert_eq!(display_name(&ALL[1], "zh"), "单人");
        assert_eq!(display_name(&ALL[2], "en"), "Group photo");
    }
}
