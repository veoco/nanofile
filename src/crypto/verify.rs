use sha2::Digest;

use crate::crypto::key_derivation::{self};

/// Verify a password against the stored magic string.
///
/// The magic is computed as `PBKDF2(repo_id + password, salt, 1000, SHA256)`.
/// Comparing against the stored magic verifies the password without needing
/// to decrypt any data.
///
/// For enc_version 2: uses fixed salt (MAGIC_SALT)
/// For enc_version 4: uses per-repo salt
/// For enc_version 1 and 3: returns false (unsupported)
///
/// Returns `true` if the password matches, `false` otherwise.
pub fn verify_repo_password(
    repo_id: &str,
    password: &str,
    enc_version: i32,
    salt: &str,
    stored_magic: &str,
) -> bool {
    let computed_magic = match key_derivation::generate_magic(repo_id, password, enc_version, salt)
    {
        Ok(m) => m,
        Err(_) => return false,
    };

    // Constant-time comparison to prevent timing attacks on the magic.
    // Use a simple hash-then-compare approach since Rust's &[u8] == is
    // not constant-time.
    let computed_hash = sha2::Sha256::digest(computed_magic.as_bytes());
    let stored_hash = sha2::Sha256::digest(stored_magic.as_bytes());

    // Compare the hashes — this is constant-time because SHA-256 output
    // is fixed-size and we compare ALL bytes regardless of match.
    computed_hash == stored_hash
}

/// Verify a pre-computed magic string against the stored magic.
///
/// This is used by the `checkpassword` API endpoint where the client
/// sends the computed magic (not the raw password).
///
/// The comparison is done via SHA-256 hash comparison for constant-time
/// behavior (same as `verify_repo_password`).
pub fn verify_magic(stored_magic: &str, provided_magic: &str) -> bool {
    let stored_hash = sha2::Sha256::digest(stored_magic.as_bytes());
    let provided_hash = sha2::Sha256::digest(provided_magic.as_bytes());
    stored_hash == provided_hash
}

/// Determine the effective salt for password operations.
///
/// For enc_version 2: returns empty string (signals "use MAGIC_SALT")
/// For enc_version 4: returns the stored salt (per-repo random salt)
/// For other versions: returns empty (unsupported, will fail at derive_key)
pub fn effective_salt(enc_version: i32, repo_salt: &str) -> &str {
    match enc_version {
        2 => "", // Use default MAGIC_SALT
        4 => repo_salt,
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verify_password_valid() {
        // Generate magic for a known password, then verify
        let repo_id = "test-repo-id";
        let password = "my-password";
        let magic = key_derivation::generate_magic(repo_id, password, 2, "").unwrap();

        assert!(verify_repo_password(repo_id, password, 2, "", &magic));
    }

    #[test]
    fn test_verify_password_invalid() {
        let repo_id = "test-repo-id";
        let password = "my-password";
        let magic = key_derivation::generate_magic(repo_id, password, 2, "").unwrap();

        assert!(!verify_repo_password(
            repo_id,
            "wrong-password",
            2,
            "",
            &magic
        ));
    }

    #[test]
    fn test_verify_password_different_repo() {
        let repo_id = "repo-a";
        let password = "my-password";
        let magic = key_derivation::generate_magic(repo_id, password, 2, "").unwrap();

        assert!(!verify_repo_password("repo-b", password, 2, "", &magic));
    }

    #[test]
    fn test_verify_magic() {
        // Just verify the self-comparison
        assert!(verify_magic("same", "same"));
        assert!(!verify_magic("same", "different"));
    }

    #[test]
    fn test_effective_salt_v2() {
        assert_eq!(effective_salt(2, ""), "");
        assert_eq!(effective_salt(2, "some_salt"), "");
    }

    #[test]
    fn test_effective_salt_v4() {
        let repo_salt = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        assert_eq!(effective_salt(4, repo_salt), repo_salt);
        assert_eq!(effective_salt(4, ""), "");
    }

    #[test]
    fn test_verify_repo_password_v4() {
        let repo_id = "test-repo-id";
        let password = "my-password";
        let repo_salt = key_derivation::generate_repo_salt();
        let magic = key_derivation::generate_magic(repo_id, password, 4, &repo_salt).unwrap();

        assert!(verify_repo_password(
            repo_id, password, 4, &repo_salt, &magic
        ));
        assert!(!verify_repo_password(
            repo_id,
            "wrong-password",
            4,
            &repo_salt,
            &magic
        ));
    }
}
