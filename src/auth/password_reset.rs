/// Password reset token generation and validation helpers.
use sha2::{Digest, Sha256};

/// Generate a raw reset token and its SHA-256 hash.
/// Returns (raw_token, token_hash).
pub fn generate_reset_token() -> (String, String) {
    let mut raw = [0u8; 32];
    rand::Rng::fill(&mut rand::thread_rng(), &mut raw);
    let raw_token = hex::encode(raw);
    let hash = hash_token(&raw_token);
    (raw_token, hash)
}

/// Compute the SHA-256 hash of a raw token for database storage.
pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

/// Token expiry: 3 days (matching seahub).
pub const RESET_TOKEN_TTL_SECONDS: i64 = 3 * 24 * 60 * 60;
