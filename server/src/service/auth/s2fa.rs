use rand::Rng;
/// Only the SHA-256 hash is stored (`repository/s2fa_token.rs` looks the token up
/// by `hash_token`), so a leaked database does not yield a usable "remember this
/// device" credential.
pub fn generate_s2fa_token() -> String {
    let mut token = [0u8; 20];
    rand::rng().fill_bytes(&mut token);
    hex::encode(token)
}

/// Default TTL for S2FA tokens: 90 days, matching seahub's
/// `TWO_FACTOR_DEVICE_REMEMBER_DAYS`. After that the next re-authentication
/// requires 2FA again even if the API token has not expired — periodic
/// re-verification of trusted devices is deliberate.
pub const S2FA_TTL_SECONDS: i64 = 90 * 24 * 60 * 60;
