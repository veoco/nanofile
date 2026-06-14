use crate::crypto::key_derivation;

/// Compute the magic string for password verification (v2 format).
///
/// The magic is the hex-encoded 32-byte key from
/// `PBKDF2(repo_id + password, MAGIC_SALT, 1000, SHA256)`.
/// Returns 64 hex chars.
///
/// For per-repo salt or different enc_version, use
/// `key_derivation::generate_magic()`.
pub fn compute_magic(repo_id: &str, password: &str) -> String {
    key_derivation::generate_magic(repo_id, password, 2, "").unwrap_or_else(|_| String::new())
}

/// Extract the key portion from a magic string (v2 format).
///
/// Expects a 64-char hex string, returns 32 bytes.
pub fn extract_key(magic: &str) -> Option<Vec<u8>> {
    key_derivation::extract_key_from_magic(magic, 2)
}

/// IV extraction from magic is not supported (the magic string only
/// contains the derived key, not the IV, in the seafile protocol).
pub fn extract_iv(_magic: &str) -> Option<Vec<u8>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_magic() {
        let magic = compute_magic("test-repo-id", "test-password");
        assert_eq!(magic.len(), 64);
        // Deterministic
        let magic2 = compute_magic("test-repo-id", "test-password");
        assert_eq!(magic, magic2);
    }

    #[test]
    fn test_extract_key_roundtrip() {
        let magic = compute_magic("repo-1", "strong-password");
        let key = extract_key(&magic).unwrap();
        assert_eq!(key.len(), 32);
    }

    #[test]
    fn test_extract_iv_returns_none() {
        assert!(extract_iv("any").is_none());
        assert!(extract_iv("").is_none());
        assert!(extract_iv("abcdef1234567890abcdef1234567890").is_none());
    }
}
