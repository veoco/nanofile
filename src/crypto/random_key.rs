use aes::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};
use rand::Rng;

type Aes256CbcEnc = cbc::Encryptor<aes::Aes256>;
type Aes256CbcDec = cbc::Decryptor<aes::Aes256>;

/// Encrypt a file block using AES-256-CBC with PKCS#7 padding.
///
/// Each block is encrypted with a random IV which is prepended to the
/// ciphertext. The output format is: `[16-byte IV][ciphertext]`.
///
/// The `file_key` is the 32-byte actual encryption key obtained from
/// `decrypt_repo_enc_key()`. Seafile always uses PKCS#7 padding regardless
/// of whether the plaintext is block-aligned.
pub fn encrypt_block(data: &[u8], file_key: &[u8]) -> Vec<u8> {
    let mut iv = [0u8; 16];
    rand::thread_rng().fill(&mut iv[..]);

    let ciphertext =
        Aes256CbcEnc::new(file_key.into(), &iv.into()).encrypt_padded_vec_mut::<Pkcs7>(data);

    let mut result = Vec::with_capacity(16 + ciphertext.len());
    result.extend_from_slice(&iv);
    result.extend_from_slice(&ciphertext);
    result
}

/// Decrypt a file block that was encrypted with `encrypt_block`.
///
/// Expects the input to be in `[16-byte IV][ciphertext]` format.
/// The `file_key` is the 32-byte actual encryption key.
pub fn decrypt_block(
    encrypted: &[u8],
    file_key: &[u8],
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if encrypted.len() < 17 {
        return Err("encrypted block too short".into());
    }

    let iv = &encrypted[..16];
    let ciphertext = &encrypted[16..];

    let plaintext = Aes256CbcDec::new(file_key.into(), iv.into())
        .decrypt_padded_vec_mut::<Pkcs7>(ciphertext)
        .map_err(|e| format!("decrypt error: {}", e))?;

    Ok(plaintext)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_block() {
        let mut key = [0u8; 32];
        rand::thread_rng().fill(&mut key[..]);

        let data = b"Hello, this is test data for block encryption!";
        let encrypted = encrypt_block(data, &key);
        let decrypted = decrypt_block(&encrypted, &key).unwrap();

        assert_eq!(data.to_vec(), decrypted);
    }

    #[test]
    fn test_encrypt_decrypt_empty_data() {
        let mut key = [0u8; 32];
        rand::thread_rng().fill(&mut key[..]);

        let data = b"";
        let encrypted = encrypt_block(data, &key);
        let decrypted = decrypt_block(&encrypted, &key).unwrap();

        assert_eq!(data.to_vec(), decrypted);
    }

    #[test]
    fn test_encrypt_decrypt_large_data() {
        let mut key = [0u8; 32];
        rand::thread_rng().fill(&mut key[..]);

        let data = vec![0xABu8; 10000];
        let encrypted = encrypt_block(&data, &key);
        let decrypted = decrypt_block(&encrypted, &key).unwrap();

        assert_eq!(data, decrypted);
    }

    #[test]
    fn test_decrypt_short_input() {
        let key = [0u8; 32];
        assert!(decrypt_block(&[0u8; 15], &key).is_err());
        assert!(decrypt_block(&[0u8; 16], &key).is_err());
    }

    #[test]
    fn test_decrypt_wrong_key() {
        let mut key1 = [0u8; 32];
        rand::thread_rng().fill(&mut key1[..]);
        let mut key2 = [0u8; 32];
        rand::thread_rng().fill(&mut key2[..]);

        let data = b"test data";
        let encrypted = encrypt_block(data, &key1);
        let result = decrypt_block(&encrypted, &key2);
        // Wrong key should either error or produce wrong data
        match result {
            Ok(decrypted) => assert_ne!(decrypted, data),
            Err(_) => {} // Error is also acceptable
        }
    }

    #[test]
    fn test_different_iv_per_call() {
        let key = [0u8; 32];
        let data = b"same data";

        let encrypted1 = encrypt_block(data, &key);
        let encrypted2 = encrypt_block(data, &key);

        // Each encryption should produce different ciphertext (different IV)
        let iv1 = &encrypted1[..16];
        let iv2 = &encrypted2[..16];
        assert_ne!(iv1, iv2);
    }

    #[test]
    fn test_block_aligned_data() {
        let mut key = [0u8; 32];
        rand::thread_rng().fill(&mut key[..]);

        // 32 bytes = exactly 2 AES blocks
        let data = b"ABCDEFGHIJKLMNOPABCDEFGHIJKLMNOP";
        assert_eq!(data.len(), 32);

        let encrypted = encrypt_block(data, &key);
        assert!(encrypted.len() > 32 + 16); // has IV + padding

        let decrypted = decrypt_block(&encrypted, &key).unwrap();
        assert_eq!(data.to_vec(), decrypted);
    }

    #[test]
    fn test_encrypt_decrypt_multi_block_unaligned() {
        let mut key = [0u8; 32];
        rand::thread_rng().fill(&mut key[..]);

        // 1001 bytes = not a multiple of 16 (AES block size)
        let data = vec![0xABu8; 1001];
        let encrypted = encrypt_block(&data, &key);
        let decrypted = decrypt_block(&encrypted, &key).unwrap();
        assert_eq!(data, decrypted);
    }

    #[test]
    fn test_encrypt_decrypt_single_byte() {
        let mut key = [0u8; 32];
        rand::thread_rng().fill(&mut key[..]);

        let data = b"\x42";
        let encrypted = encrypt_block(data, &key);
        let decrypted = decrypt_block(&encrypted, &key).unwrap();
        assert_eq!(data.to_vec(), decrypted);
    }

    #[test]
    fn test_encrypt_decrypt_max_key_bytes() {
        let key = [0xFFu8; 32]; // all bits set

        let data = b"test data with maximum key bits";
        let encrypted = encrypt_block(data, &key);
        let decrypted = decrypt_block(&encrypted, &key).unwrap();
        assert_eq!(data.to_vec(), decrypted);
    }

    #[test]
    fn test_decrypt_block_tampered() {
        let mut key = [0u8; 32];
        rand::thread_rng().fill(&mut key[..]);

        let data = b"tamper test data for block decryption";
        let encrypted = encrypt_block(data, &key);

        // Corrupt the first byte of ciphertext (after the IV)
        let mut tampered = encrypted.clone();
        tampered[16] ^= 0xFF;

        let result = decrypt_block(&tampered, &key);
        // Should either error OR produce wrong output
        match result {
            Ok(decrypted) => assert_ne!(decrypted, data),
            Err(_) => {} // error is acceptable
        }
    }
}
