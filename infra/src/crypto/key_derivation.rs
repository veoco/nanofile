use aes::cipher::{BlockModeDecrypt, BlockModeEncrypt, KeyIvInit, block_padding::Pkcs7};
use rand::Rng;
use sha2::Sha256;
use thiserror::Error;

type Aes256CbcEnc = cbc::Encryptor<aes::Aes256>;
type Aes256CbcDec = cbc::Decryptor<aes::Aes256>;

/// Fixed salt used for enc_version 1 and 2 (same as seafile-server).
/// Source: /tmp/seafile-server/common/seafile-crypt.c line 24.
const MAGIC_SALT: [u8; 8] = [0xda, 0x90, 0x45, 0xc3, 0x06, 0xc7, 0xcc, 0x26];

/// Error type for key derivation operations.
#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("unsupported encryption version: {0}")]
    UnsupportedVersion(i32),
    #[error("invalid key: {0}")]
    InvalidKey(String),
    #[error("invalid salt: {0}")]
    InvalidSalt(String),
    #[error("decryption failed: {0}")]
    DecryptionFailed(String),
    #[error("encryption failed: {0}")]
    EncryptionFailed(String),
}

/// The Seafile encryption protocol supports these versions:
///
/// | Version | Cipher       | Key Size | Salt      | KDF                        |
/// |---------|-------------|----------|-----------|----------------------------|
/// | 1       | AES-128-CBC | 16B      | static    | EVP_BytesToKey, 524288 it  |
/// | 2       | AES-256-CBC | 32B      | static    | PBKDF2-SHA256, 1000 it     |
/// | 3       | AES-128-ECB | 16B      | per-repo  | PBKDF2-SHA256, 1000 it     |
/// | 4       | AES-256-CBC | 32B      | per-repo  | PBKDF2-SHA256, 1000 it     |
///
/// For initial implementation, only versions 2 and 4 (AES-256-CBC) are supported.
/// The key derivation chain for v2/v4 (from seadroid's Crypto.java):
///
///   1. Password + repo_salt → PBKDF2(1000, SHA256) → derivedKey (32 bytes)
///   2. derivedKey + repo_salt → PBKDF2(10, SHA256) → derivedIv (16 bytes)
///   3. AES-256-CBC-Decrypt(random_key, derivedKey, derivedIv) → fileKey (32 bytes)
///   4. fileKey + salt → PBKDF2(1000, SHA256) → encKey (32 bytes, actual block key)
///   5. encKey + salt → PBKDF2(10, SHA256) → encIv (16 bytes, actual block IV)
///
/// For enc_version 2, repo_salt = MAGIC_SALT (the fixed 8-byte salt).
/// For enc_version 4, repo_salt = per-repo 32-byte random salt.
///
/// Whether a salt value means "use the default fixed salt".
fn is_default_salt(salt: &str) -> bool {
    salt.is_empty() || salt == "0000000000000000000000000000000000000000000000000000000000000000"
}

/// Decode a hex salt string to bytes.
/// Default/empty salts use the fixed MAGIC_SALT.
fn decode_salt(repo_salt: &str) -> Result<Vec<u8>, CryptoError> {
    if is_default_salt(repo_salt) {
        Ok(MAGIC_SALT.to_vec())
    } else if repo_salt.len() == 64 {
        hex::decode(repo_salt).map_err(|e| CryptoError::InvalidSalt(e.to_string()))
    } else {
        // Try as raw hex (may be shorter for v2 with MAGIC_SALT represented)
        hex::decode(repo_salt).map_err(|e| CryptoError::InvalidSalt(e.to_string()))
    }
}

/// Derive the wrapping key and IV from a password.
///
/// This is the first step of the two-layer key derivation:
/// password → (derivedKey, derivedIv) used to decrypt random_key.
///
/// For enc_version 2: uses fixed MAGIC_SALT, PBKDF2-SHA256 (1000 key + 10 IV)
/// For enc_version 4: uses per-repo salt, PBKDF2-SHA256 (1000 key + 10 IV)
/// For enc_version 1: (unsupported) would use EVP_BytesToKey with 524288 iterations
///
/// Returns (key, iv).
pub fn derive_key(
    password: &str,
    enc_version: i32,
    repo_salt: &str,
) -> Result<(Vec<u8>, Vec<u8>), CryptoError> {
    derive_key_bytes(password.as_bytes(), enc_version, repo_salt)
}

/// Derive wrapping key and IV from raw key bytes (not a password string).
///
/// This is identical to `derive_key` but accepts `&[u8]` instead of `&str`.
/// Used for the second stage of the key derivation chain where the file_key
/// is 32 raw bytes (not a UTF-8 string).
pub fn derive_key_bytes(
    key_bytes: &[u8],
    enc_version: i32,
    repo_salt: &str,
) -> Result<(Vec<u8>, Vec<u8>), CryptoError> {
    let salt_bytes = decode_salt(repo_salt)?;

    match enc_version {
        2 | 4 => {
            let mut key = [0u8; 32];
            pbkdf2::pbkdf2_hmac::<Sha256>(key_bytes, &salt_bytes, 1000, &mut key);

            let mut iv = [0u8; 16];
            pbkdf2::pbkdf2_hmac::<Sha256>(&key, &salt_bytes, 10, &mut iv);

            Ok((key.to_vec(), iv.to_vec()))
        }
        // Version 1 uses AES-128 (16-byte key) with EVP_BytesToKey and 524288 iterations
        // Version 3 uses AES-128-ECB with per-repo salt
        1 | 3 => Err(CryptoError::UnsupportedVersion(enc_version)),
        _ => Err(CryptoError::UnsupportedVersion(enc_version)),
    }
}

/// Derive the actual file encryption key by decrypting the random_key.
///
/// This is the full key derivation chain:
///   password → derive_key → (derivedKey, derivedIv)
///   AES-256-CBC-Decrypt(random_key, derivedKey, derivedIv) → fileKey (32 bytes)
///   fileKey → derive_key_bytes → (encKey, encIv)  // actual block encryption key+IV
///
/// The `random_key` parameter is a hex-encoded 96-char string (48 bytes encrypted).
/// The `salt` parameter is the per-repo salt (64 hex chars, or empty for v2).
///
/// Returns (encKey, encIv) — the actual AES key and IV for file block operations.
pub fn decrypt_repo_enc_key(
    password: &str,
    random_key: &str,
    enc_version: i32,
    repo_salt: &str,
) -> Result<(Vec<u8>, Vec<u8>), CryptoError> {
    let random_key_bytes = hex::decode(random_key)
        .map_err(|e| CryptoError::InvalidKey(format!("invalid random_key hex: {e}")))?;

    // Step 1: Derive wrapping key from password
    let (derived_key, derived_iv) = derive_key(password, enc_version, repo_salt)?;

    // Step 2: Decrypt random_key with PKCS7 to get the 32-byte secret file key
    // PKCS7 strips the padding, returning exactly the original 32-byte secret key
    let file_key = Aes256CbcDec::new_from_slices(&derived_key, &derived_iv)
        .map_err(|e| CryptoError::DecryptionFailed(format!("init: {e}")))?
        .decrypt_padded_vec::<Pkcs7>(&random_key_bytes)
        .map_err(|e| CryptoError::DecryptionFailed(format!("decrypt random_key: {e}")))?;

    // Step 3: Derive the actual block encryption key from the raw 32-byte file key
    // Pass as raw bytes — NOT via String::from_utf8_lossy which would corrupt
    // arbitrary byte values
    let (enc_key, enc_iv) = derive_key_bytes(&file_key, enc_version, repo_salt)?;

    Ok((enc_key, enc_iv))
}

/// Generate a per-repo random salt.
///
/// Returns a 64-hex-char string (32 random bytes).
/// Used for enc_version >= 3.
pub fn generate_repo_salt() -> String {
    let mut salt = [0u8; 32];
    rand::rng().fill_bytes(&mut salt);
    hex::encode(salt)
}

/// Generate a random_key (encrypted secret key) for a new encrypted repo.
///
/// This is used by clients to pre-compute the random_key locally,
/// and by the server when creating encrypted repos with a provided password.
///
/// 1. Generate a random 32-byte secret key (the actual file encryption key)
/// 2. Derive wrapping key from password
/// 3. Encrypt the secret key with AES-256-CBC + PKCS7 padding (32 → 48 bytes)
/// 4. Hex-encode the 48-byte ciphertext → 96 hex chars
///
/// Returns the hex-encoded random_key.
pub fn generate_random_key_for_repo(
    password: &str,
    enc_version: i32,
    repo_salt: &str,
) -> Result<String, CryptoError> {
    let (derived_key, derived_iv) = derive_key(password, enc_version, repo_salt)?;

    let mut secret_key = [0u8; 32]; // 32-byte random secret key
    rand::rng().fill_bytes(&mut secret_key);

    // Encrypt with PKCS7 padding: 32 bytes → 48 bytes ciphertext
    let ciphertext = Aes256CbcEnc::new_from_slices(&derived_key, &derived_iv)
        .map_err(|e| CryptoError::EncryptionFailed(format!("init: {e}")))?
        .encrypt_padded_vec::<Pkcs7>(&secret_key);

    Ok(hex::encode(&ciphertext))
}

/// Generate the magic string for password verification.
///
/// In Seafile, the magic is the hex-encoded key derived from
/// `PBKDF2(repo_id + password, salt, 1000, SHA256)`. It is used to verify
/// a password without needing to decrypt anything.
///
/// Returns a 64-hex-char string (32-byte key) for v2/v4, or 32 hex chars
/// (16-byte key) for v1. This matches the seafile-server protocol where
/// `magic` = hex(key), NOT key+iv.
pub fn generate_magic(
    repo_id: &str,
    password: &str,
    enc_version: i32,
    repo_salt: &str,
) -> Result<String, CryptoError> {
    let salt_bytes = decode_salt(repo_salt)?;
    let input = format!("{}{}", repo_id, password);

    match enc_version {
        2 | 4 => {
            let mut key = [0u8; 32];
            pbkdf2::pbkdf2_hmac::<Sha256>(input.as_bytes(), &salt_bytes, 1000, &mut key);
            Ok(hex::encode(key))
        }
        1 | 3 => Err(CryptoError::UnsupportedVersion(enc_version)),
        _ => Err(CryptoError::UnsupportedVersion(enc_version)),
    }
}

/// Extract the key portion (i.e. the entire magic) from a magic string.
///
/// For v2/v4: magic is a 64-char hex string → 32 bytes.
/// For v1: magic is a 32-char hex string → 16 bytes.
pub fn extract_key_from_magic(magic: &str, enc_version: i32) -> Option<Vec<u8>> {
    match enc_version {
        1 => {
            if magic.len() != 32 {
                return None;
            }
            hex::decode(magic).ok()
        }
        2..=4 => {
            if magic.len() != 64 {
                return None;
            }
            hex::decode(magic).ok()
        }
        _ => None,
    }
}

/// The magic string does not include IV data in seafile-server protocol.
/// This function always returns None for all versions.
pub fn extract_iv_from_magic(_magic: &str, _enc_version: i32) -> Option<Vec<u8>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_key_v2() {
        let (key, iv) = derive_key("test_password", 2, "").unwrap();
        assert_eq!(key.len(), 32);
        assert_eq!(iv.len(), 16);
    }

    #[test]
    fn test_derive_key_v4() {
        let repo_salt = generate_repo_salt();
        let (key, iv) = derive_key("test_password", 4, &repo_salt).unwrap();
        assert_eq!(key.len(), 32);
        assert_eq!(iv.len(), 16);
    }

    #[test]
    fn test_derive_key_unsupported_version() {
        assert!(derive_key("pw", 1, "").is_err());
        assert!(derive_key("pw", 3, "").is_err());
        assert!(derive_key("pw", 0, "").is_err());
        assert!(derive_key("pw", 5, "").is_err());
    }

    #[test]
    fn test_generate_repo_salt() {
        let salt1 = generate_repo_salt();
        let salt2 = generate_repo_salt();
        assert_eq!(salt1.len(), 64);
        assert_eq!(salt2.len(), 64);
        assert_ne!(salt1, salt2); // should be random
    }

    #[test]
    fn test_generate_random_key_and_decrypt() {
        let password = "my_secure_password";
        let repo_salt = generate_repo_salt();

        let random_key = generate_random_key_for_repo(password, 4, &repo_salt).unwrap();
        assert_eq!(random_key.len(), 96); // 48 bytes hex-encoded

        let (enc_key, enc_iv) = decrypt_repo_enc_key(password, &random_key, 4, &repo_salt).unwrap();
        assert_eq!(enc_key.len(), 32);
        assert_eq!(enc_iv.len(), 16);
    }

    #[test]
    fn test_generate_magic_v2() {
        let magic = generate_magic("abc123", "password", 2, "").unwrap();
        assert_eq!(magic.len(), 64);

        let key = extract_key_from_magic(&magic, 2).unwrap();
        assert_eq!(key.len(), 32);

        let iv = extract_iv_from_magic(&magic, 2);
        assert!(iv.is_none()); // magic doesn't contain IV
    }

    #[test]
    fn test_generate_magic_repeatable() {
        let magic1 = generate_magic("repo1", "pass", 2, "").unwrap();
        let magic2 = generate_magic("repo1", "pass", 2, "").unwrap();
        assert_eq!(magic1, magic2);
    }

    #[test]
    fn test_generate_magic_different_password() {
        let magic1 = generate_magic("repo1", "pass1", 2, "").unwrap();
        let magic2 = generate_magic("repo1", "pass2", 2, "").unwrap();
        assert_ne!(magic1, magic2);
    }

    #[test]
    fn test_extract_key_from_magic_v1() {
        let magic = "abcdef0123456789abcdef0123456789"; // 32 hex chars = 16 bytes
        let key = extract_key_from_magic(magic, 1).unwrap();
        assert_eq!(key.len(), 16);
    }

    #[test]
    fn test_extract_key_from_magic_v2() {
        let magic = generate_magic("r", "pw", 2, "").unwrap();
        let key = extract_key_from_magic(&magic, 2).unwrap();
        assert_eq!(key.len(), 32);
    }

    #[test]
    fn test_extract_key_from_magic_invalid() {
        assert!(extract_key_from_magic("too_short", 2).is_none());
        assert!(extract_key_from_magic("nothex__nothex__nothex__nothex__nothex__", 2).is_none());
    }

    #[test]
    fn test_derive_key_v4_with_actual_salt() {
        let repo_salt = generate_repo_salt();
        let (key, iv) = derive_key("test_password", 4, &repo_salt).unwrap();
        assert_eq!(key.len(), 32);
        assert_eq!(iv.len(), 16);
        // Verify deterministic: same salt + password = same key
        let (key2, iv2) = derive_key("test_password", 4, &repo_salt).unwrap();
        assert_eq!(key, key2);
        assert_eq!(iv, iv2);
    }

    #[test]
    fn test_generate_random_key_empty_password() {
        let random_key = generate_random_key_for_repo("", 2, "").unwrap();
        assert_eq!(random_key.len(), 96);
    }

    #[test]
    fn test_decrypt_repo_enc_key_wrong_password() {
        let password = "correct_password";
        let repo_salt = "";
        let random_key = generate_random_key_for_repo(password, 2, repo_salt).unwrap();
        let (correct_key, correct_iv) =
            decrypt_repo_enc_key(password, &random_key, 2, repo_salt).unwrap();

        // Wrong password must not produce the correct key.
        // It may return Err (PKCS7 padding mismatch, ~94%) or Ok(garbage, ~6%)
        // — either is acceptable as long as the key material doesn't match.
        if let Ok((wrong_key, wrong_iv)) =
            decrypt_repo_enc_key("wrong_password", &random_key, 2, repo_salt)
        {
            assert_ne!(wrong_key, correct_key);
            assert_ne!(wrong_iv, correct_iv);
        }

        assert_eq!(correct_key.len(), 32);
        assert_eq!(correct_iv.len(), 16);
    }
}
