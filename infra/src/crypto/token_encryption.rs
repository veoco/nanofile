//! Reversible, domain-separated encryption for per-repository sync tokens.
//!
//! Tokens must stay **recoverable**: `/api2/repo-tokens/` and download-info
//! re-present previously issued tokens, and seaf-daemon keeps using whatever token
//! it was handed (no re-authentication path on 401/403), so hashing would break
//! reuse. They are AEAD-encrypted with a key derived from `secret_key` through a
//! **distinct** HKDF info string, so a leaked database alone is not enough.
//!
//! Encryption is deterministic (all-zero nonce), so the stored value doubles as the
//! equality-lookup key; tokens are 160-bit random values, so equality is already
//! public. A [`TOKEN_CIPHER_PREFIX`] marker separates legacy plaintext rows.
//!
//! The key derivation and cipher live in [`super::aead`]; only the salt/info pair,
//! the prefix and the nonce policy are token-specific.

use super::aead::{AeadKey, domains};

/// Marker prepended to encrypted token values.
pub const TOKEN_CIPHER_PREFIX: &str = "enc1:";

/// Fixed, all-zero GCM-SIV nonce (deterministic encryption, see module docs).
const NONCE: [u8; 12] = [0u8; 12];

/// Authenticated-encryption handle for sync tokens; cheap to construct and clone.
#[derive(Clone, Debug)]
pub struct TokenCipher {
    cipher: AeadKey,
}

impl TokenCipher {
    /// Derive a [`TokenCipher`] from the server master key via HKDF-SHA256.
    pub fn from_master_key(master_key: &[u8]) -> Self {
        Self {
            cipher: AeadKey::from_master_key(master_key, domains::TOKEN_SALT, domains::TOKEN_INFO),
        }
    }

    /// Deterministically encrypt a raw token into its stored form.
    pub fn encrypt(&self, plaintext: &str) -> String {
        let ciphertext = self.cipher.encrypt(NONCE, plaintext.as_bytes());
        format!("{TOKEN_CIPHER_PREFIX}{}", hex::encode(ciphertext))
    }

    /// Decrypt a stored token value.
    ///
    /// Legacy plaintext (no prefix) is returned unchanged so the migration window keeps
    /// working. `None` means an encrypted value cannot be decrypted (e.g. the server
    /// secret changed); callers must treat that as an invalid credential.
    pub fn decrypt(&self, stored: &str) -> Option<String> {
        let Some(hex_ct) = stored.strip_prefix(TOKEN_CIPHER_PREFIX) else {
            return Some(stored.to_string());
        };
        let ciphertext = hex::decode(hex_ct).ok()?;
        let plaintext = self.cipher.decrypt(NONCE, ciphertext.as_ref()).ok()?;
        String::from_utf8(plaintext).ok()
    }

    /// Whether `stored` is already in encrypted form.
    pub fn is_encrypted(stored: &str) -> bool {
        stored.starts_with(TOKEN_CIPHER_PREFIX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cipher() -> TokenCipher {
        TokenCipher::from_master_key(b"test-master-key-material-32-bytes!")
    }

    #[test]
    fn round_trips() {
        let c = cipher();
        let token = "0123456789abcdef0123456789abcdef01234567";
        let stored = c.encrypt(token);
        assert!(TokenCipher::is_encrypted(&stored));
        assert_ne!(stored, token);
        assert_eq!(c.decrypt(&stored).as_deref(), Some(token));
    }

    #[test]
    fn legacy_plaintext_passes_through() {
        let c = cipher();
        assert_eq!(
            c.decrypt("plain-legacy-token").as_deref(),
            Some("plain-legacy-token")
        );
    }

    #[test]
    fn deterministic() {
        let c = cipher();
        assert_eq!(c.encrypt("abc"), c.encrypt("abc"));
        assert_ne!(c.encrypt("abc"), c.encrypt("abd"));
    }

    #[test]
    fn wrong_key_does_not_decrypt() {
        let a = TokenCipher::from_master_key(b"key-a-key-a-key-a-key-a-key-a-key-a!");
        let b = TokenCipher::from_master_key(b"key-b-key-b-key-b-key-b-key-b-key-b!");
        let stored = a.encrypt("secret-token");
        assert_eq!(b.decrypt(&stored), None);
    }

    #[test]
    fn malformed_ciphertext_is_none() {
        let c = cipher();
        assert_eq!(c.decrypt("enc1:not-hex"), None);
    }
}
