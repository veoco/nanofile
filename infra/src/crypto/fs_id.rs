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
