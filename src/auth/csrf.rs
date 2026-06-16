/// CSRF protection using HMAC-derived tokens.
///
/// Token = HMAC-SHA256(server_secret, session_token)
/// Stateless, derived from existing session. No extra cookies needed.
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use crate::AppState;

type HmacSha256 = Hmac<Sha256>;

/// Generate a CSRF token for the given session token.
pub fn generate_csrf_token(secret: &[u8], session_token: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC key should be valid");
    mac.update(session_token.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Validate a CSRF token against the session token.
pub fn validate_csrf_token(secret: &[u8], session_token: &str, csrf_token: &str) -> bool {
    let expected = generate_csrf_token(secret, session_token);
    expected == csrf_token
}

/// Extract the `seahub-session` token from a Cookie header string.
pub fn extract_session_token(cookie_str: &str) -> Option<&str> {
    cookie_str
        .split(';')
        .map(|s| s.trim())
        .find(|s| s.starts_with("seahub-session="))
        .and_then(|s| s.strip_prefix("seahub-session="))
}

/// Generate a CSRF token for the current session in `AppState`.
/// Returns `None` if no valid session cookie is found.
pub fn csrf_for_session(state: &AppState, cookie_str: Option<&str>) -> Option<String> {
    let session_token = cookie_str.and_then(extract_session_token)?;
    Some(generate_csrf_token(&state.csrf_secret, session_token))
}
