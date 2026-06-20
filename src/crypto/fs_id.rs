use sha1::{Digest, Sha1};

/// SHA1 hex digest — used to compute block IDs and FS object IDs.
///
/// In seafile's protocol:
/// - Block ID = sha1_hex(raw_block_data)
/// - File  ID = sha1_hex(compact_json_of_file_metadata)
/// - Dir   ID = sha1_hex(compact_json_of_dir_entry_list)
pub fn sha1_hex(data: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Async wrapper around sha1_hex via spawn_blocking.
///
/// Note: FS objects are typically <1KB JSON, and SHA1 of that is ~0.5µs.
/// The spawn_blocking overhead (~1-3µs) exceeds the computation itself,
/// so prefer the sync `sha1_hex` for small payloads. Use this only when
/// hashing large data (e.g. whole-file blocks) from an async context.
pub async fn async_sha1_hex(data: Vec<u8>) -> String {
    tokio::task::spawn_blocking(move || sha1_hex(&data))
        .await
        .unwrap_or_else(|_| {
            tracing::error!("SHA1 computation task panicked, returning empty hash");
            String::new()
        })
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
