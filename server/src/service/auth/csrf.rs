/// CSRF protection using HMAC-derived tokens.
///
/// Token = HMAC-SHA256(server_secret, session_token): stateless and derived from
/// the existing session, so no extra cookie is needed. Unauthenticated endpoints
/// (login, register, password reset) validate Origin/Referer instead.
use axum::http::HeaderMap;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

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

pub fn extract_session_token(cookie_str: &str) -> Option<&str> {
    cookie_str
        .split(';')
        .map(|s| s.trim())
        .find(|s| s.starts_with("seahub-session="))
        .and_then(|s| s.strip_prefix("seahub-session="))
}

/// Validate a form-field CSRF token for an authenticated user; missing or invalid
/// tokens are `AppError::BadRequest`.
pub fn check_form_csrf(
    state: &crate::AppState,
    session_token: &str,
    csrf_token: Option<&str>,
) -> Result<(), base::error::AppError> {
    match csrf_token {
        Some(token) => {
            if validate_csrf_token(&state.csrf_secret, session_token, token) {
                Ok(())
            } else {
                Err(base::error::AppError::BadRequest(
                    "Invalid CSRF token.".to_string(),
                ))
            }
        }
        None => Err(base::error::AppError::BadRequest(
            "CSRF token is required.".to_string(),
        )),
    }
}

/// Generate a `Set-Cookie` header string for the CSRF token cookie (`sfcsrftoken`).
///
/// The cookie is deliberately **not** HttpOnly, so JavaScript can read it and echo
/// it as an `X-CSRFToken` header; the value is the same HMAC token used for forms.
/// `max_age` should match the session cookie's `Max-Age` so both stay in sync.
pub fn csrf_cookie_header(
    secret: &[u8],
    session_token: &str,
    secure: bool,
    max_age: Option<u64>,
) -> String {
    let token = generate_csrf_token(secret, session_token);
    let mut cookie = format!("sfcsrftoken={}; Path=/; SameSite=Lax", token);
    if let Some(age) = max_age {
        cookie.push_str(&format!("; Max-Age={}", age));
    }
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

/// Validate an `X-CSRFToken` request header (the JS reads `sfcsrftoken` from its
/// cookie and echoes it back here for cookie-authenticated API calls).
pub fn validate_csrf_header(headers: &HeaderMap, secret: &[u8], session_token: &str) -> bool {
    let header_val = match headers.get("x-csrftoken").and_then(|v| v.to_str().ok()) {
        Some(v) => v,
        None => return false,
    };
    validate_csrf_token(secret, session_token, header_val)
}

/// Cookie name holding the upload-link password unlock.
pub fn upload_link_cookie_name(token: &str) -> String {
    format!("visited_ufs_{token}")
}

/// Cookie name holding the share-link password unlock.
pub fn share_link_cookie_name(token: &str) -> String {
    format!("visited_sl_{token}")
}

/// Check whether a Cookie header carries a valid password-unlock cookie.
///
/// The value is an HMAC signature over the link token (see [`generate_csrf_token`]),
/// proving the password was verified by the web form. A plaintext `=1` value is
/// rejected — that is what stops a client that never entered the password from
/// minting the cookie itself.
pub fn has_link_unlock_cookie(
    cookie_header: Option<&str>,
    cookie_name: &str,
    token: &str,
    secret: &[u8],
) -> bool {
    let prefix = format!("{cookie_name}=");
    let Some(cookie) = cookie_header else {
        return false;
    };
    let Some(value) = cookie
        .split(';')
        .map(|s| s.trim())
        .find(|s| s.starts_with(&prefix))
        .and_then(|s| s.strip_prefix(&prefix))
    else {
        return false;
    };
    validate_csrf_token(secret, token, value)
}

/// `Set-Cookie` value that unlocks a password-protected link for this browser.
///
/// Passing the password through a signed cookie instead of the URL keeps it out
/// of page links, browser history and request logs.
pub fn link_unlock_cookie(cookie_name: &str, token: &str, secret: &[u8], secure: bool) -> String {
    let sig = generate_csrf_token(secret, token);
    let mut cookie = format!("{cookie_name}={sig}; Path=/; Max-Age=86400; HttpOnly; SameSite=Lax");
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

/// Check for a valid `visited_ufs_{token}` upload-link unlock cookie; the HMAC
/// value cannot be forged by a client that never entered the password.
pub fn has_valid_upload_link_cookie(
    cookie_header: Option<&str>,
    token: &str,
    secret: &[u8],
) -> bool {
    has_link_unlock_cookie(
        cookie_header,
        &upload_link_cookie_name(token),
        token,
        secret,
    )
}

/// Validate Origin/Referer against the configured site URL origin (used where no
/// session exists for HMAC tokens). Returns `false` only when a header is present
/// and does not match; no such header (curl, non-browser clients) is accepted.
pub fn validate_origin(headers: &HeaderMap, site_url_origin: &str) -> bool {
    if let Some(origin) = headers.get("origin").and_then(|v| v.to_str().ok()) {
        return origin == site_url_origin;
    }
    if let Some(referer) = headers.get("referer").and_then(|v| v.to_str().ok()) {
        return referer.starts_with(&format!("{}/", site_url_origin)) || referer == site_url_origin;
    }
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
        assert!(validate_csrf_token(secret, "session", &token));
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
        assert!(validate_origin(&HeaderMap::new(), "https://example.com"));

        let headers = header_map_from(&[("origin", "https://example.com")]);
        assert!(validate_origin(&headers, "https://example.com"));

        let headers = header_map_from(&[("origin", "https://evil.com")]);
        assert!(!validate_origin(&headers, "https://example.com"));

        let headers = header_map_from(&[("referer", "https://example.com/some/path")]);
        assert!(validate_origin(&headers, "https://example.com"));

        let headers = header_map_from(&[("referer", "https://evil.com/")]);
        assert!(!validate_origin(&headers, "https://example.com"));

        // A lookalike subdomain referer must not match through the prefix check.
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
        let cookie = csrf_cookie_header(secret, "session-abc", false, Some(86400));
        assert!(cookie.starts_with("sfcsrftoken="));
        let value = cookie
            .trim_start_matches("sfcsrftoken=")
            .split(';')
            .next()
            .unwrap()
            .trim();
        assert_eq!(value.len(), 64, "HMAC-SHA256 hex should be 64 chars");
        assert!(cookie.contains("Path=/"), "cookie should have Path=/");
        assert!(
            cookie.contains("SameSite=Lax"),
            "cookie should have SameSite=Lax"
        );
        assert!(
            !cookie.contains("HttpOnly"),
            "CSRF cookie must not be HttpOnly"
        );
        assert!(
            !cookie.contains("Secure"),
            "CSRF cookie should not have Secure by default"
        );
        assert!(
            cookie.contains("Max-Age=86400"),
            "CSRF cookie should have Max-Age=86400"
        );
    }

    #[test]
    fn test_csrf_cookie_header_secure() {
        let secret = b"test-secret-key-12345678";
        let cookie = csrf_cookie_header(secret, "session-abc", true, None);
        assert!(
            cookie.contains("; Secure"),
            "secure=true should add Secure flag"
        );
    }

    /// Both link cookie types must gain `Secure` over HTTPS and stay usable over
    /// plain HTTP; the upload-link handler delegates here, so this covers both.
    #[test]
    fn test_link_unlock_cookie_secure_follows_site_scheme() {
        let secret = b"test-secret-key-12345678";
        for cookie_name in [
            share_link_cookie_name("tok"),
            upload_link_cookie_name("tok"),
        ] {
            let plain = link_unlock_cookie(&cookie_name, "tok", secret, false);
            let secure = link_unlock_cookie(&cookie_name, "tok", secret, true);
            assert!(
                !plain.contains("Secure"),
                "plain-HTTP link cookie must not carry Secure: {plain}"
            );
            assert!(
                secure.ends_with("; Secure"),
                "HTTPS link cookie must carry Secure: {secure}"
            );
            assert!(
                plain.contains("HttpOnly") && plain.contains("SameSite=Lax"),
                "link cookie must stay HttpOnly + SameSite=Lax: {plain}"
            );
            assert_eq!(secure.trim_end_matches("; Secure"), plain);
        }
        assert_ne!(
            share_link_cookie_name("tok"),
            upload_link_cookie_name("tok")
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
