/// CSRF protection using HMAC-derived tokens.
///
/// Token = HMAC-SHA256(server_secret, session_token)
/// Stateless, derived from existing session. No extra cookies needed.
///
/// For unauthenticated endpoints (login, register, password reset),
/// Origin/Referer header validation is used instead.
use axum::http::HeaderMap;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

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
    // Constant-time comparison to prevent timing side-channel attacks.
    expected.as_bytes().ct_eq(csrf_token.as_bytes()).into()
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

/// Validate a CSRF token from a form field for authenticated users.
/// Returns `AppError::BadRequest` if the token is missing or invalid.
pub fn check_form_csrf(
    state: &crate::AppState,
    session_token: &str,
    csrf_token: Option<&str>,
) -> Result<(), crate::error::AppError> {
    match csrf_token {
        Some(token) => {
            if validate_csrf_token(&state.csrf_secret, session_token, token) {
                Ok(())
            } else {
                Err(crate::error::AppError::BadRequest(
                    "Invalid CSRF token.".to_string(),
                ))
            }
        }
        None => Err(crate::error::AppError::BadRequest(
            "CSRF token is required.".to_string(),
        )),
    }
}

/// Generate a `Set-Cookie` header string for the CSRF token cookie (`sfcsrftoken`).
///
/// The cookie is deliberately **not** HttpOnly so JavaScript can read it and
/// include it as an `X-CSRFToken` request header.  The value is the same HMAC
/// token used for form-based CSRF protection.
pub fn csrf_cookie_header(secret: &[u8], session_token: &str, secure: bool) -> String {
    let token = generate_csrf_token(secret, session_token);
    let mut cookie = format!("sfcsrftoken={}; Path=/; SameSite=Lax", token);
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

/// Validate an `X-CSRFToken` request header against the expected HMAC token.
///
/// Used when an API endpoint is called from the browser with cookie-based
/// session authentication (the JS reads `sfcsrftoken` from its cookie and
/// echoes it back in this header).
pub fn validate_csrf_header(headers: &HeaderMap, secret: &[u8], session_token: &str) -> bool {
    let header_val = match headers.get("x-csrftoken").and_then(|v| v.to_str().ok()) {
        Some(v) => v,
        None => return false,
    };
    validate_csrf_token(secret, session_token, header_val)
}

/// Validate Origin/Referer header against the configured site URL origin.
///
/// Used for unauthenticated endpoints (login, register, password reset)
/// where no session exists for HMAC-based CSRF tokens.
/// Returns `false` only when an Origin or Referer is present AND doesn't match.
/// Returns `true` when neither header is present (curl, non-browser clients).
pub fn validate_origin(headers: &HeaderMap, site_url_origin: &str) -> bool {
    // Origin header is more reliable; check it first.
    if let Some(origin) = headers.get("origin").and_then(|v| v.to_str().ok()) {
        return origin == site_url_origin;
    }
    // Fall back to Referer header.
    if let Some(referer) = headers.get("referer").and_then(|v| v.to_str().ok()) {
        return referer.starts_with(&format!("{}/", site_url_origin)) || referer == site_url_origin;
    }
    // No Origin or Referer — accept the request (curl, CLI clients, etc.).
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_and_validate_csrf_token() {
        let secret = b"test-secret-key-12345678";
        let session = "test-session-token";
        let token = generate_csrf_token(secret, session);
        assert!(validate_csrf_token(secret, session, &token));
    }

    #[test]
    fn test_csrf_token_differs_per_session() {
        let secret = b"test-secret-key-12345678";
        let token1 = generate_csrf_token(secret, "session-a");
        let token2 = generate_csrf_token(secret, "session-b");
        assert_ne!(token1, token2);
    }

    #[test]
    fn test_csrf_token_wrong_secret_fails() {
        let token = generate_csrf_token(b"secret-1", "session");
        assert!(!validate_csrf_token(b"secret-2", "session", &token));
    }

    #[test]
    fn test_csrf_token_constant_time_compare() {
        let secret = b"test-secret-key-12345678";
        let token = generate_csrf_token(secret, "session");
        // Same validation
        assert!(validate_csrf_token(secret, "session", &token));
        // Wrong token
        assert!(!validate_csrf_token(secret, "session", "wrong-token"));
    }

    #[test]
    fn test_extract_session_token() {
        assert_eq!(
            extract_session_token("seahub-session=abc123; other=val"),
            Some("abc123")
        );
        assert_eq!(
            extract_session_token("other=val; seahub-session=xyz789"),
            Some("xyz789")
        );
        assert_eq!(extract_session_token("no-cookie-here"), None);
        assert_eq!(extract_session_token(""), None);
    }

    #[test]
    fn test_validate_origin_matching() {
        // No headers → true (curl/non-browser)
        assert!(validate_origin(&HeaderMap::new(), "https://example.com"));

        // Origin matches
        let headers = header_map_from(&[("origin", "https://example.com")]);
        assert!(validate_origin(&headers, "https://example.com"));

        // Origin does NOT match
        let headers = header_map_from(&[("origin", "https://evil.com")]);
        assert!(!validate_origin(&headers, "https://example.com"));

        // Referer matches (origin absent)
        let headers = header_map_from(&[("referer", "https://example.com/some/path")]);
        assert!(validate_origin(&headers, "https://example.com"));

        // Referer does NOT match
        let headers = header_map_from(&[("referer", "https://evil.com/")]);
        assert!(!validate_origin(&headers, "https://example.com"));

        // Referer with subdomain not treated as match (prefix check is safe)
        let headers = header_map_from(&[("referer", "https://example.com.evil.com/")]);
        assert!(!validate_origin(&headers, "https://example.com"));
    }

    fn header_map_from(kvs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (k, v) in kvs {
            map.insert(
                axum::http::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                axum::http::HeaderValue::from_str(v).unwrap(),
            );
        }
        map
    }

    #[test]
    fn test_csrf_cookie_header_basic() {
        let secret = b"test-secret-key-12345678";
        let cookie = csrf_cookie_header(secret, "session-abc", false);
        // Should start with sfcsrftoken=
        assert!(cookie.starts_with("sfcsrftoken="));
        // Extract just the token value (before the first ";")
        let value = cookie
            .trim_start_matches("sfcsrftoken=")
            .split(';')
            .next()
            .unwrap()
            .trim();
        assert_eq!(value.len(), 64, "HMAC-SHA256 hex should be 64 chars");
        // Should have Path=/ and SameSite=Lax
        assert!(cookie.contains("Path=/"), "cookie should have Path=/");
        assert!(
            cookie.contains("SameSite=Lax"),
            "cookie should have SameSite=Lax"
        );
        // Should NOT contain HttpOnly or Secure
        assert!(
            !cookie.contains("HttpOnly"),
            "CSRF cookie must not be HttpOnly"
        );
        assert!(
            !cookie.contains("Secure"),
            "CSRF cookie should not have Secure by default"
        );
    }

    #[test]
    fn test_csrf_cookie_header_secure() {
        let secret = b"test-secret-key-12345678";
        let cookie = csrf_cookie_header(secret, "session-abc", true);
        assert!(
            cookie.contains("; Secure"),
            "secure=true should add Secure flag"
        );
    }

    #[test]
    fn test_validate_csrf_header_matches() {
        let secret = b"test-secret-key-12345678";
        let session = "test-session";
        let token = generate_csrf_token(secret, session);
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::HeaderName::from_static("x-csrftoken"),
            axum::http::HeaderValue::from_str(&token).unwrap(),
        );
        assert!(validate_csrf_header(&headers, secret, session));
    }

    #[test]
    fn test_validate_csrf_header_mismatch() {
        let secret = b"test-secret-key-12345678";
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::HeaderName::from_static("x-csrftoken"),
            axum::http::HeaderValue::from_static("wrong-token-value"),
        );
        assert!(!validate_csrf_header(&headers, secret, "some-session"));
    }

    #[test]
    fn test_validate_csrf_header_missing() {
        let secret = b"test-secret-key-12345678";
        let headers = HeaderMap::new();
        assert!(!validate_csrf_header(&headers, secret, "test-session"));
    }

    #[test]
    fn test_extract_session_multiple_cookies() {
        let cookie = "seahub-session=my-token; other=cookie; seahub-session=second";
        assert_eq!(extract_session_token(cookie), Some("my-token"));
    }
}
