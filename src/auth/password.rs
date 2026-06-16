use rand::Rng;
use sha2::Sha256;

const SALT_LEN: usize = 16;
const HASH_LEN: usize = 32;
const LEGACY_ITERATIONS: u32 = 1000;

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

/// Verify a password against a stored hash.
///
/// First tries with the provided `iterations` count. If that fails, falls
/// back to 1000 iterations for backward compatibility with hashes created
/// before the configurable-iterations feature.
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

    // Try with the current iteration count first.
    let computed = pbkdf2_hash(password.as_bytes(), &salt, iterations);
    if computed.as_slice() == stored_hash.as_slice() {
        return true;
    }

    // Fall back to legacy iterations (1000) for old hashes.
    if iterations != LEGACY_ITERATIONS {
        let legacy = pbkdf2_hash(password.as_bytes(), &salt, LEGACY_ITERATIONS);
        return legacy.as_slice() == stored_hash.as_slice();
    }

    false
}

/// Hash with legacy (1000) iterations — convenience for callers that don't
/// have access to the config (e.g. CLI adduser, share link password).
pub fn hash_password_legacy(password: &str) -> String {
    hash_password(password, LEGACY_ITERATIONS)
}

/// Verify with legacy (1000) iterations — convenience for callers that don't
/// have access to the config.
pub fn verify_password_legacy(password: &str, password_hash: &str) -> bool {
    verify_password(password, password_hash, LEGACY_ITERATIONS)
}

/// Validate password strength.
/// Returns Ok(()) if the password meets the configured requirements.
pub fn validate_password(
    password: &str,
    min_length: u32,
    require_strong: bool,
) -> Result<(), String> {
    if (password.len() as u32) < min_length {
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
