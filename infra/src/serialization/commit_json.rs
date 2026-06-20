use sha1::{Digest, Sha1};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CommitData {
    pub commit_id: String,
    pub repo_id: String,
    pub root_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub creator_name: String,
    pub creator: String,
    pub description: String,
    pub ctime: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub second_parent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_desc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypted: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enc_version: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub magic: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub version: i32,
}

impl CommitData {
    pub fn to_compact_json(&self) -> String {
        serde_json::to_string(self).unwrap()
    }

    pub fn compute_commit_id(&self) -> String {
        let mut hasher = Sha1::new();
        hasher.update(self.root_id.as_bytes());
        hasher.update(self.creator.as_bytes());
        hasher.update(self.creator_name.as_bytes());
        hasher.update(self.description.as_bytes());
        let ctime_be = self.ctime.to_be_bytes();
        hasher.update(ctime_be);
        hex::encode(hasher.finalize())
    }
}
