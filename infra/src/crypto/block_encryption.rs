//! Server-side transparent at-rest encryption for file blocks (AES-256-GCM-SIV).
//!
//! Unlike the client-side Seafile CBC encryption in [`super::random_key`], this
//! module encrypts every block at rest on the server. It chooses AES-GCM-SIV
//! because:
//!
//! - **Deterministic**: encrypting the same plaintext twice yields the same
//!   ciphertext, so content-addressed block dedup (`block_id = sha1(logical
//!   bytes)`) still works. Blocks are content-addressed anyway, so "these two
//!   blocks are equal" is already public information.
//! - **Authenticated**: a 128-bit tag is appended, so on-disk tampering is
//!   detected on read. Under `lazy` migration mode the tag check doubles as a
//!   cheap discriminator between newly-encrypted and legacy plaintext blocks.
//! - **Length-preserving**: GCM-SIV adds no padding; `ciphertext.len() ==
//!   plaintext.len() + 16`, so logical sizes are recoverable without reading
//!   the whole block (see the store wrapper).
//!
//! The 12-byte nonce is fixed to all-zeros. GCM-SIV is nonce-misuse-resistant:
//! reusing a nonce only leaks whether two plaintexts are equal — which is
//! already public via content addressing.

use aes_gcm_siv::aead::{Aead, KeyInit};
use aes_gcm_siv::{Aes256GcmSiv, Nonce};

/// Number of bytes appended to a ciphertext as the authentication tag.
pub const TAG_LEN: usize = 16;

/// Fixed, all-zero GCM-SIV nonce. Deterministic encryption (see module docs).
const NONCE: [u8; 12] = [0u8; 12];

/// HKDF salt used for domain separation from the raw master key.
const HKDF_SALT: &[u8] = b"nanofile-v1";
/// HKDF info string selecting the block-data-encryption key.
const HKDF_INFO_BLOCK_DEK: &[u8] = b"nanofile/block-dek/v1";
/// Length of the derived block data-encryption key (AES-256).
const DEK_LEN: usize = 32;

/// A symmetric authenticated-encryption handle for server-side block at-rest
/// encryption. Cheap to construct and safe to clone; holds the 32-byte DEK.
#[derive(Clone)]
pub struct BlockCipher {
    cipher: Aes256GcmSiv,
}

impl std::fmt::Debug for BlockCipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never render the key material.
        f.debug_struct("BlockCipher").finish_non_exhaustive()
    }
}

impl BlockCipher {
    /// Derive a [`BlockCipher`] from the server master key via HKDF-SHA256.
    ///
    /// The master key is ≥ 32 bytes of high entropy held out-of-band (env /
    /// `*_FILE`). No random salt is mixed in, keeping encryption deterministic
    /// so content-addressed dedup is preserved.
    pub fn from_master_key(master_key: &[u8]) -> Self {
        let hkdf = hkdf::Hkdf::<sha2::Sha256>::new(Some(HKDF_SALT), master_key);
        let mut dek = [0u8; DEK_LEN];
        hkdf.expand(HKDF_INFO_BLOCK_DEK, &mut dek)
            .expect("DEK length is within HKDF output bound");
        Self {
            cipher: Aes256GcmSiv::new_from_slice(&dek)
                .expect("32-byte key is valid for AES-256-GCM-SIV"),
        }
    }

    /// Encrypt `plaintext`, returning `plaintext || 16-byte tag`.
    pub fn encrypt(&self, plaintext: &[u8]) -> Vec<u8> {
        let nonce = Nonce::from(NONCE);
        self.cipher
            .encrypt(&nonce, plaintext)
            .expect("GCM-SIV encryption never fails for a fixed nonce")
    }

    /// Decrypt a value produced by [`BlockCipher::encrypt`]. Returns `Err` on
    /// tag mismatch (tampered or wrong-key data).
    pub fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>, aes_gcm_siv::aead::Error> {
        let nonce = Nonce::from(NONCE);
        self.cipher.decrypt(&nonce, ciphertext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> [u8; 32] {
        [0x42; 32]
    }

    #[test]
    fn derive_is_deterministic() {
        let k1 = BlockCipher::from_master_key(&test_key());
        let k2 = BlockCipher::from_master_key(&test_key());
        assert_eq!(k1.encrypt(b"data"), k2.encrypt(b"data"));
    }

    #[test]
    fn derive_differs_for_different_master_keys() {
        let a = BlockCipher::from_master_key(&test_key());
        let b = BlockCipher::from_master_key(&[0x99u8; 32]);
        assert_ne!(a.encrypt(b"data"), b.encrypt(b"data"));
    }

    #[test]
    fn key_derivation_matches_reference_vector() {
        // RFC 5869 test case 1 (SHA-256), expanded to a 42-byte output.
        let ikm = [0x0bu8; 22];
        let salt = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c,
        ];
        let info = [0xf0u8, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9];
        let hkdf = hkdf::Hkdf::<sha2::Sha256>::new(Some(&salt), &ikm);
        let mut out = [0u8; 42];
        hkdf.expand(&info, &mut out).unwrap();
        assert_eq!(
            out.as_slice(),
            &hex::decode(
                "3cb25f25faacd57a90434f64d0362f2a\
                 2d2d0a90cf1a5a4c5db02d56ecc4c5bf\
                 34007208d5b887185865"
            )
            .unwrap()
        );
    }

    #[test]
    fn roundtrip_encrypt_decrypt() {
        let c = BlockCipher::from_master_key(&test_key());
        for data in [
            b"".as_slice(),
            b"x".as_slice(),
            b"Hello, GCM-SIV block encryption!".as_slice(),
            &[0xABu8; 1000],
        ] {
            let ct = c.encrypt(data);
            assert_eq!(ct.len(), data.len() + TAG_LEN);
            assert_eq!(c.decrypt(&ct).unwrap(), data);
        }
    }

    #[test]
    fn deterministic_encryption_equal_plaintexts() {
        let c = BlockCipher::from_master_key(&test_key());
        assert_eq!(c.encrypt(b"same"), c.encrypt(b"same"));
    }

    #[test]
    fn tampered_tag_fails() {
        let c = BlockCipher::from_master_key(&test_key());
        let data = b"tamper test";
        let mut ct = c.encrypt(data);
        ct[0] ^= 0xFF;
        assert!(c.decrypt(&ct).is_err());
    }

    #[test]
    fn wrong_key_fails() {
        let c1 = BlockCipher::from_master_key(&[0x11u8; 32]);
        let c2 = BlockCipher::from_master_key(&[0x22u8; 32]);
        let ct = c1.encrypt(b"secret");
        assert!(c2.decrypt(&ct).is_err());
    }

    #[test]
    fn too_short_ciphertext_fails() {
        let c = BlockCipher::from_master_key(&test_key());
        assert!(c.decrypt(&[]).is_err());
        assert!(c.decrypt(&[0u8; TAG_LEN - 1]).is_err());
    }
}
