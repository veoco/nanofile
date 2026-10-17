use sha1::{Digest, Sha1};

/// SHA1 hex digest for block and FS object IDs.
///
/// Block ID = sha1_hex(raw bytes); file ID = sha1_hex(compact JSON metadata);
/// dir ID = sha1_hex(compact JSON dir-entry list).
pub fn sha1_hex(data: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}
