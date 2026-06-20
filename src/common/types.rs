use serde::Serialize;

/// Directory entry returned by API listing endpoints (used by both api and api_v21).
#[derive(Serialize)]
pub struct DirEntry {
    pub id: String,
    #[serde(rename = "type")]
    pub entry_type: String,
    pub name: String,
    pub size: i64,
    pub mtime: i64,
    pub permission: String,
    /// Last modifier email (empty string if unknown). Files only in the
    /// original seafile protocol, but we store it for all entry types.
    #[serde(default)]
    pub modifier: String,
    /// Parent directory path. Present only in recursive listings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_dir: Option<String>,
    /// Modifier display name. Present only for file entries in recursive listings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modifier_name: Option<String>,
    /// Modifier contact email. Present only for file entries in recursive listings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modifier_contact_email: Option<String>,
}
