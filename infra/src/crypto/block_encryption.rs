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
//! - **Length-preserving**: GCM-SIV adds no padding, so the logical size never
//!   needs a full decrypt — only the leading bytes that identify the format.
//!   Versioned ciphertext is `plaintext.len() + 16 + 6` (tag + `NFE1 || key_id`
//!   header); the header-less legacy format is `plaintext.len() + 16`. Sizing a
//!   block by the versioned constant alone under-reports legacy blocks by six
//!   bytes, which shifts every offset a client derives from it (see
//!   [`BlockCipher::plaintext_len`]).
//!
//! The 12-byte nonce is fixed to all-zeros. GCM-SIV is nonce-misuse-resistant:
//! reusing a nonce only leaks whether two plaintexts are equal — which is
//! already public via content addressing.

use aes_gcm_siv::aead::{Aead, KeyInit};
use aes_gcm_siv::{Aes256GcmSiv, Nonce};
use zeroize::Zeroize;

/// Number of bytes appended to a ciphertext as the authentication tag.
pub const TAG_LEN: usize = 16;

/// Magic prefix identifying a versioned (`NFE1`) ciphertext.
pub const MAGIC: &[u8; 4] = b"NFE1";
/// Active key id. Only `0` exists today (no rotation support), but the field
/// lets a future key-rotation scheme identify which key encrypted a block.
pub const ACTIVE_KEY_ID: u16 = 0;
/// Length of the versioned header: magic (4) + key id (2).
pub const HEADER_LEN: usize = 6;

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
        let cipher =
            Aes256GcmSiv::new_from_slice(&dek).expect("32-byte key is valid for AES-256-GCM-SIV");
        // The key schedule inside `Aes256GcmSiv` cannot be scrubbed, but the
        // raw DEK buffer should not linger on the stack either.
        dek.zeroize();
        Self { cipher }
    }

    /// Encrypt `plaintext`, returning `NFE1 || key_id || ciphertext || tag`.
    ///
    /// The versioned header lets a future key rotation identify the key that
    /// produced a block; `decrypt` still accepts header-less legacy ciphertext.
    pub fn encrypt(&self, plaintext: &[u8]) -> Vec<u8> {
        let nonce = Nonce::from(NONCE);
        let body = self
            .cipher
            .encrypt(&nonce, plaintext)
            .expect("GCM-SIV encryption never fails for a fixed nonce");
        let mut out = Vec::with_capacity(HEADER_LEN + body.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&ACTIVE_KEY_ID.to_be_bytes());
        out.extend_from_slice(&body);
        out
    }

    /// Whether `stored` carries the versioned (`NFE1 || key_id`) header, i.e.
    /// was written by this cipher rather than being pre-encryption plaintext.
    ///
    /// Callers that must tolerate legacy plaintext blocks (the `lazy`
    /// migration mode) use this to decide *by format* whether a decryption
    /// failure means "legacy plaintext" or "corrupt/tampered ciphertext" — a
    /// failure on a block that does have the header must never be mistaken for
    /// plaintext.
    pub fn looks_encrypted(stored: &[u8]) -> bool {
        stored.len() >= HEADER_LEN && &stored[..4] == MAGIC
    }

    /// Logical (plaintext) length of a stored block, derived from its on-disk
    /// format alone — no decryption.
    ///
    /// `stored_prefix` may be the whole stored block or just its leading
    /// [`HEADER_LEN`] bytes (only the magic decides the format) while
    /// `stored_len` is the block's real on-disk length, so a caller can size a
    /// large block from a short read.
    ///
    /// Versioned ciphertext carries the header *and* the tag; the header-less
    /// legacy format carries only the tag. Assuming the versioned overhead for
    /// a header-less block therefore under-reports it by [`HEADER_LEN`] — which
    /// is exactly what the store wrapper must not do, because clients compute
    /// block offsets from the sizes it reports.
    ///
    /// Returns `Err` for an unknown versioned key id, or when `stored_len`
    /// cannot hold the overhead the format implies (a truncated or non-cipher
    /// block), so a reported size can never come from an underflow.
    ///
    /// A header-less block is treated as legacy *ciphertext* (tag only). That
    /// matches `On` mode, where every read decrypts; a header-less block that
    /// held bare plaintext would fail authentication anyway.
    pub fn plaintext_len(
        stored_prefix: &[u8],
        stored_len: usize,
    ) -> Result<usize, aes_gcm_siv::aead::Error> {
        let overhead = if Self::looks_encrypted(stored_prefix) {
            let key_id = u16::from_be_bytes([stored_prefix[4], stored_prefix[5]]);
            if key_id != ACTIVE_KEY_ID {
                return Err(aes_gcm_siv::aead::Error);
            }
            HEADER_LEN + TAG_LEN
        } else {
            TAG_LEN
        };
        stored_len
            .checked_sub(overhead)
            .ok_or(aes_gcm_siv::aead::Error)
    }

    /// Decrypt a value produced by [`BlockCipher::encrypt`].
    ///
    /// Accepts both the versioned (`NFE1 || key_id`) format written by this
    /// version and the header-less legacy format from earlier deployments.
    /// Returns `Err` on tag mismatch (tampered, wrong key) or an unknown key id.
    pub fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>, aes_gcm_siv::aead::Error> {
        let nonce = Nonce::from(NONCE);
        if Self::looks_encrypted(ciphertext) {
            let key_id = u16::from_be_bytes([ciphertext[4], ciphertext[5]]);
            if key_id != ACTIVE_KEY_ID {
                return Err(aes_gcm_siv::aead::Error);
            }
            self.cipher.decrypt(&nonce, &ciphertext[HEADER_LEN..])
        } else {
            // Legacy ciphertext written before the versioned header existed.
            self.cipher.decrypt(&nonce, ciphertext)
        }
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
            assert_eq!(ct.len(), data.len() + HEADER_LEN + TAG_LEN);
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
    fn plaintext_len_matches_both_stored_formats() {
        let c = BlockCipher::from_master_key(&test_key());
        let data = b"format sizing".as_slice();

        // Versioned ciphertext: header + body, so the overhead is header + tag.
        let versioned = c.encrypt(data);
        assert_eq!(versioned.len(), data.len() + HEADER_LEN + TAG_LEN);
        assert_eq!(
            BlockCipher::plaintext_len(&versioned, versioned.len()).unwrap(),
            data.len()
        );
        // A short prefix is enough: only the magic decides the format.
        assert_eq!(
            BlockCipher::plaintext_len(&versioned[..HEADER_LEN], versioned.len()).unwrap(),
            data.len()
        );

        // Header-less legacy ciphertext: tag only, so the logical size is the
        // stored size minus TAG_LEN (not minus the versioned overhead).
        let legacy = versioned[HEADER_LEN..].to_vec();
        assert_eq!(
            BlockCipher::plaintext_len(&legacy, legacy.len()).unwrap(),
            data.len()
        );

        // Truncated block / unknown key id are errors, never underflowed sizes.
        assert!(BlockCipher::plaintext_len(&legacy[..TAG_LEN - 1], TAG_LEN - 1).is_err());
        let mut foreign = versioned.clone();
        foreign[5] = 0xFF;
        assert!(BlockCipher::plaintext_len(&foreign, foreign.len()).is_err());
    }

    #[test]
    fn too_short_ciphertext_fails() {
        let c = BlockCipher::from_master_key(&test_key());
        assert!(c.decrypt(&[]).is_err());
        assert!(c.decrypt(&[0u8; TAG_LEN - 1]).is_err());
    }

    #[test]
    fn versioned_header_and_legacy_compat() {
        let c = BlockCipher::from_master_key(&test_key());
        let data = b"versioned";
        let ct = c.encrypt(data);
        assert_eq!(&ct[..4], MAGIC);
        assert_eq!(u16::from_be_bytes([ct[4], ct[5]]), ACTIVE_KEY_ID);
        assert_eq!(c.decrypt(&ct).unwrap(), data);

        // Header-less legacy ciphertext (raw GCM-SIV output) still decrypts.
        let nonce = Nonce::from(NONCE);
        let legacy = c.cipher.encrypt(&nonce, data.as_slice()).unwrap();
        assert_ne!(&legacy[..4], MAGIC);
        assert_eq!(c.decrypt(&legacy).unwrap(), data);

        // An unknown key id is rejected rather than mis-decrypted.
        let mut foreign = ct.clone();
        foreign[5] = 0xFF;
        assert!(c.decrypt(&foreign).is_err());
    }
}
