//! Reversible, domain-separated encryption for per-repository sync tokens.
//!
//! Sync tokens must stay **recoverable**: `/api2/repo-tokens/` and
//! `/api2/repos/{id}/download-info/` re-present previously issued tokens to the
//! client, and seaf-daemon keeps using whatever token it was last handed (it
//! has no re-authentication path on 401/403). Hashing would therefore break
//! token reuse, so tokens are encrypted with an AEAD key derived from the
//! server `secret_key` through a **distinct** HKDF info string: a leaked
//! database alone is not enough to recover them without the server secret.
//!
//! Encryption is deterministic (fixed all-zero nonce), so the stored value can
//! double as the indexed equality-lookup key — mirroring
//! [`super::block_encryption`]. Sync tokens are 160-bit random values, so
//! leaking whether two tokens are equal is already public information.
//!
//! Stored values carry a [`TOKEN_CIPHER_PREFIX`] marker so the migration window
//! (rows written before this change) is unambiguous: values without the prefix
//! are legacy plaintext and are returned as-is.

use aes_gcm_siv::aead::{Aead, KeyInit};
use aes_gcm_siv::{Aes256GcmSiv, Nonce};
use zeroize::Zeroize;

/// Marker prepended to encrypted token values.
pub const TOKEN_CIPHER_PREFIX: &str = "enc1:";

/// HKDF salt used for domain separation from the raw master key.
const HKDF_SALT: &[u8] = b"nanofile-token-v1";
/// HKDF info string selecting the sync-token data-encryption key.
const HKDF_INFO_TOKEN_DEK: &[u8] = b"nanofile/sync-token-dek/v1";
/// Length of the derived data-encryption key (AES-256).
const DEK_LEN: usize = 32;
/// Fixed, all-zero GCM-SIV nonce (deterministic encryption, see module docs).
const NONCE: [u8; 12] = [0u8; 12];

/// A symmetric authenticated-encryption handle for sync tokens.
///
/// Cheap to construct and safe to clone; holds the 32-byte DEK.
#[derive(Clone)]
pub struct TokenCipher {
    cipher: Aes256GcmSiv,
}

impl std::fmt::Debug for TokenCipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never render the key material.
        f.debug_struct("TokenCipher").finish_non_exhaustive()
    }
}

impl TokenCipher {
    /// Derive a [`TokenCipher`] from the server master key via HKDF-SHA256.
    pub fn from_master_key(master_key: &[u8]) -> Self {
        let hkdf = hkdf::Hkdf::<sha2::Sha256>::new(Some(HKDF_SALT), master_key);
        let mut dek = [0u8; DEK_LEN];
        hkdf.expand(HKDF_INFO_TOKEN_DEK, &mut dek)
            .expect("DEK length is within HKDF output bound");
        let cipher =
            Aes256GcmSiv::new_from_slice(&dek).expect("32-byte key is valid for AES-256-GCM-SIV");
        // The key schedule inside `Aes256GcmSiv` cannot be scrubbed, but the
        // raw DEK buffer should not linger on the stack either.
        dek.zeroize();
        Self { cipher }
    }

    /// Deterministically encrypt a raw token into its stored form.
    pub fn encrypt(&self, plaintext: &str) -> String {
        let nonce = Nonce::from(NONCE);
        let ciphertext = self
            .cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .expect("GCM-SIV encryption never fails for a fixed nonce");
        format!("{TOKEN_CIPHER_PREFIX}{}", hex::encode(ciphertext))
    }

    /// Decrypt a stored token value.
    ///
    /// Legacy plaintext values (no prefix) are returned unchanged so the
    /// migration window keeps working. Returns `None` when an encrypted value
    /// cannot be decrypted (e.g. the server secret changed) — callers must
    /// treat that as an invalid credential, not an internal error.
    pub fn decrypt(&self, stored: &str) -> Option<String> {
        let Some(hex_ct) = stored.strip_prefix(TOKEN_CIPHER_PREFIX) else {
            return Some(stored.to_string());
        };
        let ciphertext = hex::decode(hex_ct).ok()?;
        let nonce = Nonce::from(NONCE);
        let plaintext = self.cipher.decrypt(&nonce, ciphertext.as_ref()).ok()?;
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
