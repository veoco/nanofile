use rand::Rng;
use sha2::Sha256;
use subtle::ConstantTimeEq;

const SALT_LEN: usize = 16;
const HASH_LEN: usize = 32;

/// Algorithm tag of the versioned hash format written by [`hash_password`].
const HASH_ALGO: &str = "pbkdf2_sha256";

/// Upper bound accepted when reading an iteration count out of a stored hash, so a
/// corrupted (or hand-edited) row cannot make verification spin unbounded.
const MAX_ITERATIONS: u32 = 10_000_000;

/// A syntactically valid hash that can never match, used to equalize login latency
/// when the user does not exist: otherwise a "user not found" response returns
/// instantly while a wrong-password one runs PBKDF2, leaking which emails are
/// registered through a response-time side channel.
///
/// The cost must match the configured cost, so it is embedded in the value rather
/// than baked into a constant: a fixed cheap count would re-open the side channel.
pub fn dummy_password_hash(iterations: u32) -> String {
    format!(
        "{HASH_ALGO}${iterations}${}${}",
        "0".repeat(SALT_LEN * 2),
        "0".repeat(HASH_LEN * 2)
    )
}

fn pbkdf2_hash(password: &[u8], salt: &[u8], iterations: u32) -> [u8; HASH_LEN] {
    let mut key = [0u8; HASH_LEN];
    pbkdf2::pbkdf2_hmac::<Sha256>(password, salt, iterations, &mut key);
    key
}

struct ParsedHash {
    iterations: u32,
    salt: Vec<u8>,
    hash: Vec<u8>,
}

/// Parse a stored hash.
///
/// Current format: `pbkdf2_sha256$<iterations>$<hex salt>$<hex hash>`.
/// Legacy format: `hex(salt):hex(hash)`, whose cost is not recorded and is
/// supplied by the caller.
fn parse_hash(password_hash: &str) -> Option<ParsedHash> {
    if let Some(rest) = password_hash.strip_prefix(&format!("{HASH_ALGO}$")) {
        let mut parts = rest.splitn(3, '$');
        let iterations: u32 = parts.next()?.parse().ok()?;
        if iterations == 0 || iterations > MAX_ITERATIONS {
            return None;
        }
        let salt = hex::decode(parts.next()?).ok()?;
        let hash = hex::decode(parts.next()?).ok()?;
        return Some(ParsedHash {
            iterations,
            salt,
            hash,
        });
    }

    let (salt_hex, hash_hex) = password_hash.split_once(':')?;
    Some(ParsedHash {
        iterations: 0, // filled in by the caller
        salt: hex::decode(salt_hex).ok()?,
        hash: hex::decode(hash_hex).ok()?,
    })
}

/// Hash a password with the given iteration count.
///
/// The count is stored with the hash, so raising `auth.password_hash_iterations`
/// only affects newly written hashes instead of invalidating existing ones.
pub fn hash_password(password: &str, iterations: u32) -> String {
    let mut salt = [0u8; SALT_LEN];
    rand::rng().fill_bytes(&mut salt);

    let hash = pbkdf2_hash(password.as_bytes(), &salt, iterations);
    format!(
        "{HASH_ALGO}${iterations}${}${}",
        hex::encode(salt),
        hex::encode(hash)
    )
}

/// Async wrapper running PBKDF2 off the tokio executor: hashing is CPU-bound and
/// would otherwise stall a worker thread and delay unrelated requests.
pub async fn verify_password_async(
    password: String,
    password_hash: String,
    iterations: u32,
) -> bool {
    tokio::task::spawn_blocking(move || verify_password(&password, &password_hash, iterations))
        .await
        .unwrap_or(false)
}

/// Async wrapper around [`hash_password`]; see [`verify_password_async`].
///
/// A `spawn_blocking` join error must not degrade into an empty hash: the login
/// path uses this to re-hash a legacy password on a *successful* login, and a
/// broken hash would lock the account out. Fall back to hashing inline instead.
pub async fn hash_password_async(password: String, iterations: u32) -> String {
    let inline = password.clone();
    match tokio::task::spawn_blocking(move || hash_password(&password, iterations)).await {
        Ok(hash) => hash,
        Err(e) => {
            tracing::error!("password hashing task failed ({e}); hashing inline");
            hash_password(&inline, iterations)
        }
    }
}

/// Verify a password against a stored hash using constant-time comparison.
///
/// The count embedded in `password_hash` wins; `fallback_iterations` is only used
/// for legacy hashes that predate the embedded count.
pub fn verify_password(password: &str, password_hash: &str, fallback_iterations: u32) -> bool {
    let Some(parsed) = parse_hash(password_hash) else {
        return false;
    };
    let iterations = if parsed.iterations == 0 {
        fallback_iterations
    } else {
        parsed.iterations
    };

    let computed = pbkdf2_hash(password.as_bytes(), &parsed.salt, iterations);
    // Constant-time comparison to prevent timing side-channel attacks.
    computed.as_slice().ct_eq(parsed.hash.as_slice()).into()
}

/// Whether a stored hash should be rewritten with `current_iterations`.
///
/// True for legacy hashes (no recorded cost) and for hashes whose recorded cost
/// no longer matches. Callers rewrite on successful login, upgrading the whole
/// user base without a migration or a forced reset.
pub fn needs_rehash(password_hash: &str, current_iterations: u32) -> bool {
    match parse_hash(password_hash) {
        Some(parsed) => parsed.iterations == 0 || parsed.iterations != current_iterations,
        None => false,
    }
}

/// Validate password strength; `Ok(())` when the configured requirements are met.
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
        assert!(validate_password("密码测试", 4, false).is_ok());
    }

    #[test]
    fn test_validate_password_unicode_byte_count_larger_than_char_count() {
        assert!(validate_password("密码测a", 4, false).is_ok());
        assert!(validate_password("密码测a", 5, false).is_err());
    }

    #[test]
    fn test_validate_password_emoji_count() {
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
        // Versioned format with a broken/nonsensical cost is rejected without
        // running PBKDF2 for an unbounded time.
        assert!(!verify_password("password", "pbkdf2_sha256$0$aa$bb", 1000));
        assert!(!verify_password(
            "password",
            "pbkdf2_sha256$99999999999$aa$bb",
            1000
        ));
        assert!(!verify_password(
            "password",
            "pbkdf2_sha256$abc$aa$bb",
            1000
        ));
        assert!(!verify_password(
            "password",
            "pbkdf2_sha256$1000$zz$bb",
            1000
        ));
    }

    /// The stored cost must win over the current setting.
    #[test]
    fn test_embedded_cost_survives_config_change() {
        let password = "test";
        let hash_1k = hash_password(password, 1000);
        assert!(verify_password(password, &hash_1k, 1000));
        assert!(
            verify_password(password, &hash_1k, 2000),
            "the stored cost must win over the current setting"
        );
        assert!(hash_1k.starts_with("pbkdf2_sha256$1000$"));
    }

    /// Hashes written before the cost was embedded stay verifiable.
    #[test]
    fn test_legacy_hash_still_verifies() {
        let password = "legacy_password";
        let salt = [7u8; SALT_LEN];
        let legacy = format!(
            "{}:{}",
            hex::encode(salt),
            hex::encode(pbkdf2_hash(password.as_bytes(), &salt, 1000))
        );
        assert!(verify_password(password, &legacy, 1000));
        assert!(!verify_password(password, &legacy, 2000));
        assert!(!verify_password("wrong", &legacy, 1000));
    }

    #[test]
    fn test_needs_rehash() {
        let modern = hash_password("pw", 1000);
        assert!(!needs_rehash(&modern, 1000));
        assert!(needs_rehash(&modern, 2000), "cost changed");
        let salt = [1u8; SALT_LEN];
        let legacy = format!(
            "{}:{}",
            hex::encode(salt),
            hex::encode(pbkdf2_hash(b"pw", &salt, 1000))
        );
        assert!(needs_rehash(&legacy, 1000));
        assert!(!needs_rehash("garbage", 1000));
    }

    /// The not-found dummy must cost the same as a real hash at the current
    /// setting, otherwise the response-time side channel reopens.
    #[test]
    fn test_dummy_hash_uses_current_cost() {
        let dummy = dummy_password_hash(1000);
        assert!(dummy.starts_with("pbkdf2_sha256$1000$"));
        assert!(!verify_password("anything", &dummy, 1000));
        assert!(!needs_rehash(&dummy, 1000));
        assert!(needs_rehash(&dummy, 2000));
    }
}
