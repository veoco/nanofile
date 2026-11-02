//! The three automatic tags and their stable keys.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LabelDef {
    pub key: &'static str,
    pub zh: &'static str,
    pub en: &'static str,
    pub color: &'static str,
}

pub const PERSON_ANY: &str = "person_any";
pub const PERSON_SINGLE: &str = "person_single";
pub const PERSON_GROUP: &str = "person_group";

pub const ALL: &[LabelDef] = &[
    LabelDef {
        key: PERSON_ANY,
        zh: "人物",
        en: "People",
        color: "#3b82f6",
    },
    LabelDef {
        key: PERSON_SINGLE,
        zh: "单人",
        en: "Single person",
        color: "#06b6d4",
    },
    LabelDef {
        key: PERSON_GROUP,
        zh: "合影",
        en: "Group photo",
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

/// Display name in the configured UI language. Tag names are data, so a shared
/// repository keeps whatever was created; search aliases cover both languages.
pub fn display_name(label: &LabelDef, language: &str) -> &'static str {
    if language.to_ascii_lowercase().starts_with("zh") {
        label.zh
    } else {
        label.en
    }
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
}
