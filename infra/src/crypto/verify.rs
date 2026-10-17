use sha2::Digest;
use subtle::ConstantTimeEq;

use crate::crypto::key_derivation::{self};

/// Verify a password against the stored magic.
///
/// The magic is `PBKDF2(repo_id + password, salt, 1000, SHA256)`; matching it
/// verifies the password without decrypting data. v2 uses the fixed salt, v4 the
/// per-repo salt; v1/v3 are unsupported and return false.
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

    // Constant-time comparison, because `==` on byte slices short-circuits.
    // Hash first so the comparison length is fixed.
    let computed_hash = sha2::Sha256::digest(computed_magic.as_bytes());
    let stored_hash = sha2::Sha256::digest(stored_magic.as_bytes());

    // Constant-time comparison: `==` short-circuits and leaks timing.
    computed_hash
        .as_slice()
        .ct_eq(stored_hash.as_slice())
        .into()
}

/// Verify a pre-computed magic (the `checkpassword` endpoint sends the magic, not the
/// password). Compares SHA-256 digests for constant time, like `verify_repo_password`.
pub fn verify_magic(stored_magic: &str, provided_magic: &str) -> bool {
    let stored_hash = sha2::Sha256::digest(stored_magic.as_bytes());
    let provided_hash = sha2::Sha256::digest(provided_magic.as_bytes());
    stored_hash
        .as_slice()
        .ct_eq(provided_hash.as_slice())
        .into()
}

/// Effective salt: empty for v2 (signals [`MAGIC_SALT`]) and for unsupported
/// versions; the stored per-repo salt for v4.
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
