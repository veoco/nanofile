/// Generate a 40-character hex S2FA device trust token.
use rand::Rng;
///
/// Only the SHA-256 hash is stored (`repository/s2fa_token.rs` looks the token
/// up by `hash_token`), so a leaked database does not yield a usable
/// "remember this device" credential.
pub fn generate_s2fa_token() -> String {
    let mut token = [0u8; 20];
    rand::rng().fill_bytes(&mut token);
    hex::encode(token)
}

/// Default TTL for S2FA tokens: 90 days.
///
/// This matches the official seahub setting:
///   seahub/settings.py:TWO_FACTOR_DEVICE_REMEMBER_DAYS = 90
///
/// S2FA defines a trust window: for 90 days from first login, the
/// device can re-authenticate without being prompted for 2FA.
/// After 90 days, the next re-authentication event requires 2FA
/// again, even if the API token hasn't expired. This is a deliberate
/// security design — periodic re-verification of trusted devices.
pub const S2FA_TTL_SECONDS: i64 = 90 * 24 * 60 * 60;
