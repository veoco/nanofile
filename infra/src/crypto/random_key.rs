use aes::cipher::{BlockModeDecrypt, BlockModeEncrypt, KeyIvInit, block_padding::Pkcs7};

type Aes256CbcEnc = cbc::Encryptor<aes::Aes256>;
type Aes256CbcDec = cbc::Decryptor<aes::Aes256>;

/// Encrypt a file block using AES-256-CBC with PKCS7 padding (Seafile protocol).
///
/// Seafile protocol: the IV is deterministic (derived from the key chain),
/// and no IV is prepended to the ciphertext. The stored block is raw
/// AES-256-CBC ciphertext with PKCS7 padding.
///
/// Block ID = SHA-1(ciphertext), matching seafile's seafile_encrypt().
///
/// Encrypt a file block with AES-256-CBC (Seafile protocol).
///
/// # Panics
/// Panics if `file_key` is not 32 bytes or `file_iv` is not 16 bytes.
/// Callers must always pass valid key/IV lengths (enforced by the Seafile
/// protocol — key is always 32 bytes derived via PBKDF2, IV is always 16 bytes).
pub fn encrypt_block(data: &[u8], file_key: &[u8], file_iv: &[u8]) -> Vec<u8> {
    debug_assert!(
        file_key.len() == 32,
        "AES-256 key must be 32 bytes, got {}",
        file_key.len()
    );
    debug_assert!(
        file_iv.len() == 16,
        "AES IV must be 16 bytes, got {}",
        file_iv.len()
    );
    Aes256CbcEnc::new_from_slices(file_key, file_iv)
        .expect("key must be 32 bytes, IV must be 16 bytes")
        .encrypt_padded_vec::<Pkcs7>(data)
}

/// Decrypt a file block encrypted with `encrypt_block` (Seafile protocol).
///
/// Expects raw AES-256-CBC ciphertext (no IV prefix). The IV is deterministic
/// from the key chain, passed as `file_iv`.
pub fn decrypt_block(
    encrypted: &[u8],
    file_key: &[u8],
    file_iv: &[u8],
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if encrypted.is_empty() {
        return Err("encrypted block is empty".into());
    }

    let plaintext = Aes256CbcDec::new_from_slices(file_key, file_iv)
        .map_err(|e| format!("decrypt init error: {}", e))?
        .decrypt_padded_vec::<Pkcs7>(encrypted)
        .map_err(|e| format!("decrypt error: {}", e))?;

    Ok(plaintext)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::Rng;

    fn random_key_iv() -> ([u8; 32], [u8; 16]) {
        let mut key = [0u8; 32];
        let mut iv = [0u8; 16];
        rand::rng().fill_bytes(&mut key);
        rand::rng().fill_bytes(&mut iv);
        (key, iv)
    }

    #[test]
    fn test_encrypt_decrypt_block() {
        let (key, iv) = random_key_iv();
        let data = b"Hello, this is test data for block encryption!";
        let encrypted = encrypt_block(data, &key, &iv);
        let decrypted = decrypt_block(&encrypted, &key, &iv).unwrap();
        assert_eq!(data.to_vec(), decrypted);
    }

    #[test]
    fn test_encrypt_decrypt_empty_data() {
        let (key, iv) = random_key_iv();
        let data = b"";
        let encrypted = encrypt_block(data, &key, &iv);
        let decrypted = decrypt_block(&encrypted, &key, &iv).unwrap();
        assert_eq!(data.to_vec(), decrypted);
    }

    #[test]
    fn test_encrypt_decrypt_large_data() {
        let (key, iv) = random_key_iv();
        let data = vec![0xABu8; 10000];
        let encrypted = encrypt_block(&data, &key, &iv);
        let decrypted = decrypt_block(&encrypted, &key, &iv).unwrap();
        assert_eq!(data, decrypted);
    }

    #[test]
    fn test_decrypt_short_input() {
        let key = [0u8; 32];
        let iv = [0u8; 16];
        assert!(decrypt_block(&[], &key, &iv).is_err());
    }

    #[test]
    fn test_decrypt_wrong_key() {
        let (key1, iv1) = random_key_iv();
        let (key2, iv2) = random_key_iv();

        let data = b"test data";
        let encrypted = encrypt_block(data, &key1, &iv1);
        let result = decrypt_block(&encrypted, &key2, &iv2);
        // Wrong key should either error or produce wrong data
        if let Ok(decrypted) = result {
            assert_ne!(decrypted, data);
        }
        // Error is also acceptable
    }

    #[test]
    fn test_deterministic_encryption() {
        let (key, iv) = random_key_iv();
        let data = b"same data";

        let encrypted1 = encrypt_block(data, &key, &iv);
        let encrypted2 = encrypt_block(data, &key, &iv);

        // Same IV + same key + same data = same ciphertext (Seafile protocol)
        assert_eq!(encrypted1, encrypted2);
    }

    #[test]
    fn test_block_aligned_data() {
        let (key, iv) = random_key_iv();

        // 32 bytes = exactly 2 AES blocks
        let data = b"ABCDEFGHIJKLMNOPABCDEFGHIJKLMNOP";
        assert_eq!(data.len(), 32);

        let encrypted = encrypt_block(data, &key, &iv);
        // With PKCS7 padding, 32 bytes input → 48 bytes output (2 blocks + 1 padding block)
        assert_eq!(encrypted.len(), 48);

        let decrypted = decrypt_block(&encrypted, &key, &iv).unwrap();
        assert_eq!(data.to_vec(), decrypted);
    }

    #[test]
    fn test_encrypt_decrypt_multi_block_unaligned() {
        let (key, iv) = random_key_iv();

        // 1001 bytes = not a multiple of 16 (AES block size)
        let data = vec![0xABu8; 1001];
        let encrypted = encrypt_block(&data, &key, &iv);
        let decrypted = decrypt_block(&encrypted, &key, &iv).unwrap();
        assert_eq!(data, decrypted);
    }

    #[test]
    fn test_encrypt_decrypt_single_byte() {
        let (key, iv) = random_key_iv();

        let data = b"\x42";
        let encrypted = encrypt_block(data, &key, &iv);
        let decrypted = decrypt_block(&encrypted, &key, &iv).unwrap();
        assert_eq!(data.to_vec(), decrypted);
    }

    #[test]
    fn test_encrypt_decrypt_max_key_bytes() {
        let key = [0xFFu8; 32]; // all bits set
        let iv = [0xFFu8; 16];

        let data = b"test data with maximum key bits";
        let encrypted = encrypt_block(data, &key, &iv);
        let decrypted = decrypt_block(&encrypted, &key, &iv).unwrap();
        assert_eq!(data.to_vec(), decrypted);
    }

    #[test]
    fn test_decrypt_block_tampered() {
        let (key, iv) = random_key_iv();

        let data = b"tamper test data for block decryption";
        let encrypted = encrypt_block(data, &key, &iv);

        // Corrupt the first byte of ciphertext (no IV prefix now, so offset 0)
        let mut tampered = encrypted.clone();
        tampered[0] ^= 0xFF;

        let result = decrypt_block(&tampered, &key, &iv);
        // Should either error OR produce wrong output
        if let Ok(decrypted) = result {
            assert_ne!(decrypted, data);
        }
    }
}
