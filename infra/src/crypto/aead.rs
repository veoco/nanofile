//! Shared AES-256-GCM-SIV construction for the three encrypted-at-rest values.
//!
//! Sync tokens, TOTP seeds and blocks each derived their own key and built their
//! own cipher; the bodies were byte-identical apart from one HKDF constant, which
//! left domain separation enforced only by convention. This module owns the
//! derivation and the cipher handle; the callers keep their storage formats and
//! nonce policies.

use aes_gcm_siv::Aes256GcmSiv;
use aes_gcm_siv::aead::{Aead, KeyInit};
use zeroize::Zeroize;

/// The HKDF salt/info pairs that keep the three key domains separate.
///
/// A leaked master key is not enough to cross domains only because these values
/// differ; the domain-separation test derives all three DEKs and compares them.
pub(crate) mod domains {
    pub(crate) const TOKEN_SALT: &[u8] = b"nanofile-token-v1";
    pub(crate) const TOKEN_INFO: &[u8] = b"nanofile/sync-token-dek/v1";
    pub(crate) const TOTP_SALT: &[u8] = b"nanofile-totp-v1";
    pub(crate) const TOTP_INFO: &[u8] = b"nanofile/totp-seed-dek/v1";
    pub(crate) const BLOCK_SALT: &[u8] = b"nanofile-v1";
    pub(crate) const BLOCK_INFO: &[u8] = b"nanofile/block-dek/v1";
}

/// Derive a 32-byte data-encryption key from the master key via HKDF-SHA256.
pub(crate) fn derive_dek(master_key: &[u8], salt: &[u8], info: &[u8]) -> [u8; 32] {
    let hkdf = hkdf::Hkdf::<sha2::Sha256>::new(Some(salt), master_key);
    let mut dek = [0u8; 32];
    hkdf.expand(info, &mut dek)
        .expect("DEK length is within HKDF output bound");
    dek
}

/// Authenticated-encryption handle derived from one master key and domain.
#[derive(Clone)]
pub(crate) struct AeadKey {
    cipher: Aes256GcmSiv,
}

impl std::fmt::Debug for AeadKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never render the key material.
        f.debug_struct("AeadKey").finish_non_exhaustive()
    }
}

impl AeadKey {
    /// Derive the handle from `master_key` in the domain named by `salt`/`info`.
    pub(crate) fn from_master_key(master_key: &[u8], salt: &[u8], info: &[u8]) -> Self {
        let mut dek = derive_dek(master_key, salt, info);
        let cipher =
            Aes256GcmSiv::new_from_slice(&dek).expect("32-byte key is valid for AES-256-GCM-SIV");
        // The cipher's key schedule cannot be scrubbed, clear the raw DEK buffer.
        dek.zeroize();
        Self { cipher }
    }

    pub(crate) fn encrypt(&self, nonce: [u8; 12], plaintext: &[u8]) -> Vec<u8> {
        self.cipher
            .encrypt(&nonce.into(), plaintext)
            .expect("GCM-SIV encryption never fails for a 12-byte nonce")
    }

    pub(crate) fn decrypt(
        &self,
        nonce: [u8; 12],
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, aes_gcm_siv::aead::Error> {
        self.cipher.decrypt(&nonce.into(), ciphertext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three domains must derive independent keys: a future edit that copies a
    /// salt/info pair between ciphers would make one ciphertext decrypt under
    /// another domain's key (and break the at-rest separation).
    #[test]
    fn the_three_domains_derive_independent_keys() {
        let master = [0x5Au8; 32];
        let token = derive_dek(&master, domains::TOKEN_SALT, domains::TOKEN_INFO);
        let totp = derive_dek(&master, domains::TOTP_SALT, domains::TOTP_INFO);
        let block = derive_dek(&master, domains::BLOCK_SALT, domains::BLOCK_INFO);

        assert_ne!(token, totp);
        assert_ne!(token, block);
        assert_ne!(totp, block);
        assert_ne!(domains::TOKEN_SALT, domains::TOTP_SALT);
        assert_ne!(domains::TOKEN_INFO, domains::TOTP_INFO);
        assert_ne!(domains::BLOCK_SALT, domains::TOKEN_SALT);
        assert_ne!(domains::BLOCK_INFO, domains::TOTP_INFO);
    }

    #[test]
    fn derivation_is_deterministic() {
        let master = [0x11u8; 32];
        assert_eq!(
            derive_dek(&master, domains::TOKEN_SALT, domains::TOKEN_INFO),
            derive_dek(&master, domains::TOKEN_SALT, domains::TOKEN_INFO)
        );
    }
}
