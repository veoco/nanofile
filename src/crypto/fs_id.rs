use sha1::{Digest, Sha1};

pub fn compute_fs_id(data: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

pub fn compute_commit_id(
    repo_id: &str,
    root_id: &str,
    parent_id: Option<&str>,
    ctime: i64,
    creator_name: &str,
    description: &str,
) -> String {
    let mut hasher = Sha1::new();
    hasher.update(repo_id.as_bytes());
    hasher.update(root_id.as_bytes());
    if let Some(parent) = parent_id {
        hasher.update(parent.as_bytes());
    }
    hasher.update(ctime.to_string().as_bytes());
    hasher.update(creator_name.as_bytes());
    hasher.update(description.as_bytes());
    hex::encode(hasher.finalize())
}

pub fn compute_block_id(data: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}
