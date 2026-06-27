use rand::Rng;
use sha2::Sha256;
use subtle::ConstantTimeEq;

const SALT_LEN: usize = 16;
const HASH_LEN: usize = 32;

fn pbkdf2_hash(password: &[u8], salt: &[u8], iterations: u32) -> [u8; HASH_LEN] {
    let mut key = [0u8; HASH_LEN];
    pbkdf2::pbkdf2_hmac::<Sha256>(password, salt, iterations, &mut key);
    key
}

/// Hash a password with the given iteration count.
/// Format: `hex(salt):hex(hash)`.
pub fn hash_password(password: &str, iterations: u32) -> String {
    let mut salt = [0u8; SALT_LEN];
    rand::rng().fill_bytes(&mut salt);

    let hash = pbkdf2_hash(password.as_bytes(), &salt, iterations);
    format!("{}:{}", hex::encode(salt), hex::encode(hash))
}

/// Verify a password against a stored hash using constant-time comparison.
pub fn verify_password(password: &str, password_hash: &str, iterations: u32) -> bool {
    let parts: Vec<&str> = password_hash.splitn(2, ':').collect();
    if parts.len() != 2 {
        return false;
    }

    let salt = match hex::decode(parts[0]) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let stored_hash = match hex::decode(parts[1]) {
        Ok(h) => h,
        Err(_) => return false,
    };

    let computed = pbkdf2_hash(password.as_bytes(), &salt, iterations);
    // Constant-time comparison to prevent timing side-channel attacks.
    computed.as_slice().ct_eq(stored_hash.as_slice()).into()
}

/// Validate password strength.
/// Returns Ok(()) if the password meets the configured requirements.
pub fn validate_password(
    password: &str,
    min_length: u32,
    require_strong: bool,
) -> Result<(), String> {
    if (password.chars().count() as u32) < min_length {
        return Err(format!(
            "Password must be at least {} characters long.",
            min_length
        ));
    }
    if require_strong {
        let has_letter = password.chars().any(|c| c.is_ascii_alphabetic());
        let has_digit = password.chars().any(|c| c.is_ascii_digit());
        if !has_letter || !has_digit {
            return Err("Password must contain at least one letter and one digit.".to_string());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_password_ascii_meets_min_length() {
        assert!(validate_password("abcd1234", 8, false).is_ok());
    }

    #[test]
    fn test_validate_password_ascii_below_min_length() {
        assert!(validate_password("abc123", 8, false).is_err());
    }

    #[test]
    fn test_validate_password_unicode_meets_min_length() {
        // 4 Chinese characters, each 3 bytes in UTF-8 → 12 bytes, 4 chars
        assert!(validate_password("密码测试", 4, false).is_ok());
    }

    #[test]
    fn test_validate_password_unicode_byte_count_larger_than_char_count() {
        // 3 Chinese chars + 1 ASCII = 4 chars but 10 bytes
        // Should pass min_length=4 (char count)
        assert!(validate_password("密码测a", 4, false).is_ok());
        // Should fail min_length=5 (only 4 chars)
        assert!(validate_password("密码测a", 5, false).is_err());
    }

    #[test]
    fn test_validate_password_emoji_count() {
        // 4 emoji characters (each 4 bytes) = 16 bytes, 4 chars
        assert!(validate_password("😀🎉🚀💡", 4, false).is_ok());
    }

    #[test]
    fn test_validate_password_strong_requires_letter_and_digit() {
        assert!(validate_password("abcdefgh", 8, true).is_err());
        assert!(validate_password("12345678", 8, true).is_err());
        assert!(validate_password("abcd1234", 8, true).is_ok());
    }

    #[test]
    fn test_hash_and_verify_roundtrip() {
        let password = "test_password_123";
        let hash = hash_password(password, 1000);
        assert!(verify_password(password, &hash, 1000));
        assert!(!verify_password("wrong_password", &hash, 1000));
    }

    #[test]
    fn test_verify_invalid_hash_format() {
        assert!(!verify_password("password", "invalid-hash", 1000));
        assert!(!verify_password("password", "not:hex:hash", 1000));
    }

    #[test]
    fn test_hash_different_iterations() {
        let password = "test";
        let hash_1k = hash_password(password, 1000);
        assert!(verify_password(password, &hash_1k, 1000));
        // verify with wrong iterations does not match
        assert!(!verify_password(password, &hash_1k, 2000));
    }
}
