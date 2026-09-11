//! At-rest encryption for TOTP second-factor seeds.
//!
//! A TOTP seed is password-equivalent: anyone who reads it can mint valid codes
//! forever, so a leaked database would silently defeat 2FA for every enrolled
//! user. The seed is never used as a lookup key, so — unlike sync tokens — it is
//! encrypted with a **random nonce per value**, which removes the equality leak
//! that deterministic encryption would expose.
//!
//! The key is derived from the server `secret_key` through a distinct HKDF info
//! string, so the database alone is not enough to recover seeds.
//!
//! Stored values carry a [`TOTP_CIPHER_PREFIX`] marker; anything without it is
//! legacy plaintext and is returned unchanged, which keeps the migration window
//! (rows written before this change) working.

use aes_gcm_siv::aead::{Aead, KeyInit};
use aes_gcm_siv::{Aes256GcmSiv, Nonce};
use rand::Rng;
use zeroize::Zeroize;

/// Marker prepended to encrypted TOTP seeds.
pub const TOTP_CIPHER_PREFIX: &str = "totp1:";

/// HKDF salt, distinct from the sync-token salt.
const HKDF_SALT: &[u8] = b"nanofile-totp-v1";
/// HKDF info string selecting the TOTP data-encryption key.
const HKDF_INFO_TOTP_DEK: &[u8] = b"nanofile/totp-seed-dek/v1";
/// Length of the derived data-encryption key (AES-256).
const DEK_LEN: usize = 32;
/// Per-value random nonce length (GCM-SIV accepts any length).
const NONCE_LEN: usize = 12;

/// A symmetric authenticated-encryption handle for TOTP seeds.
///
/// Cheap to construct and safe to clone; holds the 32-byte DEK.
#[derive(Clone)]
pub struct TotpCipher {
    cipher: Aes256GcmSiv,
}

impl std::fmt::Debug for TotpCipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never render the key material.
        f.debug_struct("TotpCipher").finish_non_exhaustive()
    }
}

impl TotpCipher {
    /// Derive a [`TotpCipher`] from the server master key via HKDF-SHA256.
    pub fn from_master_key(master_key: &[u8]) -> Self {
        let hkdf = hkdf::Hkdf::<sha2::Sha256>::new(Some(HKDF_SALT), master_key);
        let mut dek = [0u8; DEK_LEN];
        hkdf.expand(HKDF_INFO_TOTP_DEK, &mut dek)
            .expect("DEK length is within HKDF output bound");
        let cipher =
            Aes256GcmSiv::new_from_slice(&dek).expect("32-byte key is valid for AES-256-GCM-SIV");
        // The key schedule inside `Aes256GcmSiv` cannot be scrubbed, but the
        // raw DEK buffer should not linger on the stack either.
        dek.zeroize();
        Self { cipher }
    }

    /// Encrypt a base32 seed into its stored form.
    pub fn encrypt(&self, plaintext: &str) -> String {
        let mut nonce_bytes = [0u8; NONCE_LEN];
        rand::rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from(nonce_bytes);
        let ciphertext = self
            .cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .expect("GCM-SIV encryption never fails for a fresh nonce");
        format!(
            "{TOTP_CIPHER_PREFIX}{}:{}",
            hex::encode(nonce_bytes),
            hex::encode(ciphertext)
        )
    }

    /// Decrypt a stored seed.
    ///
    /// Legacy plaintext values (no prefix) are returned unchanged. Returns
    /// `None` when an encrypted value cannot be decrypted — for example after
    /// the server secret was rotated. Callers must treat that as "this user has
    /// to re-enrol", and never as a successful verification.
    pub fn decrypt(&self, stored: &str) -> Option<String> {
        let Some(rest) = stored.strip_prefix(TOTP_CIPHER_PREFIX) else {
            return Some(stored.to_string());
        };
        let (nonce_hex, ct_hex) = rest.split_once(':')?;
        let nonce_bytes: [u8; NONCE_LEN] = hex::decode(nonce_hex).ok()?.try_into().ok()?;
        let ciphertext = hex::decode(ct_hex).ok()?;
        let nonce = Nonce::from(nonce_bytes);
        let plaintext = self.cipher.decrypt(&nonce, ciphertext.as_ref()).ok()?;
        String::from_utf8(plaintext).ok()
    }

    /// Whether `stored` is already in encrypted form.
    pub fn is_encrypted(stored: &str) -> bool {
        stored.starts_with(TOTP_CIPHER_PREFIX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cipher() -> TotpCipher {
        TotpCipher::from_master_key(b"test-master-key-material-32-bytes!")
    }

    #[test]
    fn round_trips() {
        let c = cipher();
        let seed = "JBSWY3DPEHPK3PXP";
        let stored = c.encrypt(seed);
        assert!(TotpCipher::is_encrypted(&stored));
        assert!(!stored.contains(seed), "seed must not be stored in clear");
        assert_eq!(c.decrypt(&stored).as_deref(), Some(seed));
    }

    #[test]
    fn legacy_plaintext_passes_through() {
        let c = cipher();
        assert_eq!(
            c.decrypt("JBSWY3DPEHPK3PXP").as_deref(),
            Some("JBSWY3DPEHPK3PXP")
        );
    }

    #[test]
    fn nonce_is_random_per_value() {
        let c = cipher();
        let a = c.encrypt("SAME");
        let b = c.encrypt("SAME");
        assert_ne!(a, b, "a fixed nonce would leak seed equality");
        assert_eq!(c.decrypt(&a).as_deref(), Some("SAME"));
        assert_eq!(c.decrypt(&b).as_deref(), Some("SAME"));
    }

    #[test]
    fn wrong_key_does_not_decrypt() {
        let a = TotpCipher::from_master_key(b"key-a-key-a-key-a-key-a-key-a-key-a!");
        let b = TotpCipher::from_master_key(b"key-b-key-b-key-b-key-b-key-b-key-b!");
        let stored = a.encrypt("JBSWY3DPEHPK3PXP");
        assert_eq!(b.decrypt(&stored), None);
    }

    #[test]
    fn tampered_ciphertext_is_none() {
        let c = cipher();
        let stored = c.encrypt("JBSWY3DPEHPK3PXP");
        let mut tampered = stored.clone();
        tampered.push('0');
        assert_eq!(c.decrypt(&tampered), None);
    }

    #[test]
    fn malformed_ciphertext_is_none() {
        let c = cipher();
        assert_eq!(c.decrypt("totp1:not-hex"), None);
        assert_eq!(c.decrypt("totp1:aabb:not-hex"), None);
    }
}
