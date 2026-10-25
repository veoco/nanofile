use sha1::{Digest, Sha1};

/// SHA1 hex digest for block and FS object IDs.
///
/// Block ID = sha1_hex(raw bytes); file ID = sha1_hex(compact JSON metadata);
/// dir ID = sha1_hex(compact JSON dir-entry list).
pub fn sha1_hex(data: &[u8]) -> String {
    hex::encode(sha1_bytes(data))
}

/// The raw 20-byte SHA1 digest.
///
/// Callers that only key an in-memory map use this to avoid allocating a hex
/// string per lookup.
pub fn sha1_bytes(data: &[u8]) -> [u8; 20] {
    let mut hasher = Sha1::new();
    hasher.update(data);
    hasher.finalize().into()
}
