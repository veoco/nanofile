//! SHA-256 hashing of bearer tokens for database storage.
//!
//! Tokens are 160-bit random values, so a fast hash suffices: the purpose is only
//! to keep raw tokens out of a leaked database, which is infeasible to invert.
//! The repository layer hashes at the storage boundary, so this lives with the
//! other crypto primitives rather than with a server-side service.

use sha2::{Digest, Sha256};

/// Compute the SHA-256 hash of a bearer token for database storage.
pub fn sha256_hex(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hash_is_64_lowercase_hex_chars() {
        let hash = sha256_hex("some-token-value");
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(hash, hash.to_lowercase());
    }

    #[test]
    fn hashing_is_deterministic_and_distinct() {
        assert_eq!(sha256_hex("token-a"), sha256_hex("token-a"));
        assert_ne!(sha256_hex("token-a"), sha256_hex("token-b"));
    }
}
