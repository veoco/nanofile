//! Axum middleware and extractors — authentication, permission checking.

pub mod auth;
pub mod repo_extractor;

use std::net::SocketAddr;

use crate::AppState;
use base::error::AppError;

use axum::http::{HeaderValue, header};

/// Add baseline security headers to every response.
///
/// `script-src 'self'` — no `'unsafe-inline'`: the pre-paint preference guards
/// load as a blocking external script, the share pages share one bundle, and
/// the translation table travels as a `type="application/json"` data block,
/// which is not executable and therefore not subject to `script-src`.
///
/// `style-src` still allows `'unsafe-inline'`: the public share pages carry
/// their own inline `<style>` block, and tag colours are runtime data rendered
/// as `style="background-color: ..."` attributes. Tightening it needs those
/// moved to CSS classes first.
///
/// Everything else is locked to `'self'` (no fallback to `*`), so remote
/// script, frame, font and object loading is blocked.
///
/// `Strict-Transport-Security` is only sent when `site_url` is HTTPS: a
/// plain-HTTP LAN deployment must not be pinned to HTTPS by its own server.
pub async fn security_headers(
    axum::extract::State(state): axum::extract::State<std::sync::Arc<crate::AppState>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let mut response = next.run(req).await;
    apply_security_headers(
        response.headers_mut(),
        state.config.server.secure_cookies(),
        state.config.server.hsts_include_subdomains,
    );
    response
}

/// Insert the baseline security headers.
///
/// `secure` is `site_url` being HTTPS; only then is `Strict-Transport-Security`
/// added, because a plain-HTTP deployment must not pin clients to a scheme its
/// own links do not use. `hsts_include_subdomains` extends that pin to
/// subdomains and is opt-in, since a deployment may serve unrelated plain-HTTP
/// sites on sibling host names.
fn apply_security_headers(
    headers: &mut axum::http::HeaderMap,
    secure: bool,
    hsts_include_subdomains: bool,
) {
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        header::HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
    // Nothing here is meant to be embedded by another origin (X-Frame-Options
    // and `frame-ancestors` already cover framing; this also blocks subresource
    // embedding of authenticated responses).
    headers.insert(
        header::HeaderName::from_static("cross-origin-resource-policy"),
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; script-src 'self'; \
             style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; \
             font-src 'self'; connect-src 'self'; \
             object-src 'none'; frame-ancestors 'none'; base-uri 'self'; \
             form-action 'self'",
        ),
    );
    if secure {
        // `includeSubDomains` is opt-in: a deployment may serve unrelated
        // plain-HTTP sites on sibling host names.
        headers.insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static(if hsts_include_subdomains {
                "max-age=31536000; includeSubDomains"
            } else {
                "max-age=31536000"
            }),
        );
    }
}

/// Enforce CSRF protection for a state-changing handler registered on `GET`.
///
/// [`AuthUser`](auth::AuthUser) deliberately skips its CSRF check for safe
/// methods, because a browser cannot attach a custom header to an `<img>` or
/// `<link>` subresource. A handler that mutates state while living on `GET`
/// therefore has to demand the token itself, otherwise a cross-site subresource
/// request carrying the victim's session cookie is enough to perform the write.
///
/// Bearer-token callers are left alone: a cross-site request cannot set an
/// `Authorization` header, so they are not CSRF-able.
pub fn require_csrf_for_cookie_session(
    headers: &axum::http::HeaderMap,
    csrf_secret: &[u8],
) -> Result<(), AppError> {
    if headers.contains_key(axum::http::header::AUTHORIZATION) {
        return Ok(());
    }
    let Some(cookie) = headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
    else {
        return Ok(());
    };
    let Some(session) = crate::service::auth::csrf::extract_session_token(cookie) else {
        return Ok(());
    };
    if crate::service::auth::csrf::validate_csrf_header(headers, csrf_secret, session) {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

/// Reject a request when external share/upload links are globally disabled by
/// the `server.share_link_enabled` setting. Call this at the top of handlers
/// that create or serve anonymous share/upload links.
pub fn ensure_share_links_enabled(state: &std::sync::Arc<AppState>) -> Result<(), AppError> {
    if state.config.server.share_link_enabled {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

/// Determine the effective client IP for rate limiting.
///
/// Uses the TCP peer address exposed via `ConnectInfo`. `X-Forwarded-For` is
/// only consulted when the peer is a configured `trusted_proxies` entry.
///
/// The list is then walked **right to left**, skipping entries that are
/// themselves trusted proxies, and the first address that is not one of ours is
/// returned. Taking the leftmost entry instead would hand the value to the
/// client: the common proxy configuration appends to `X-Forwarded-For`
/// (`$proxy_add_x_forwarded_for`), so a forged `X-Forwarded-For: 1.2.3.4` sent
/// by an attacker would sit to the left of the real client address and every
/// per-IP limit keyed on it becomes attacker-chosen.
pub fn effective_client_ip(
    addr: &SocketAddr,
    headers: &axum::http::HeaderMap,
    trusted_proxies: &[String],
) -> String {
    let peer = addr.ip().to_string();
    if !trusted_proxies.iter().any(|p| p == &peer) {
        return peer;
    }

    let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) else {
        return peer;
    };

    for hop in xff.split(',').rev() {
        let hop = hop.trim();
        if hop.is_empty() || trusted_proxies.iter().any(|p| p == hop) {
            continue;
        }
        return hop.to_string();
    }

    // Every hop was one of our own proxies (or the header was empty): fall back
    // to the peer rather than to an attacker-supplied value.
    peer
}

#[cfg(test)]
mod security_header_tests {
    use super::apply_security_headers;

    fn headers(secure: bool) -> axum::http::HeaderMap {
        headers_with_subdomains(secure, false)
    }

    fn headers_with_subdomains(
        secure: bool,
        hsts_include_subdomains: bool,
    ) -> axum::http::HeaderMap {
        let mut headers = axum::http::HeaderMap::new();
        apply_security_headers(&mut headers, secure, hsts_include_subdomains);
        headers
    }

    /// HSTS is only meaningful over HTTPS: sending it from a plain-HTTP LAN
    /// deployment would pin clients to a scheme the server cannot serve.
    #[test]
    fn hsts_follows_the_site_scheme() {
        assert_eq!(
            headers(true).get("strict-transport-security").unwrap(),
            "max-age=31536000"
        );
        assert!(
            headers(false).get("strict-transport-security").is_none(),
            "plain-HTTP deployments must not receive HSTS"
        );
        // `includeSubDomains` is opt-in and only ever added to an HTTPS pin.
        assert_eq!(
            headers_with_subdomains(true, true)
                .get("strict-transport-security")
                .unwrap(),
            "max-age=31536000; includeSubDomains"
        );
        assert!(
            headers_with_subdomains(false, true)
                .get("strict-transport-security")
                .is_none(),
            "a plain-HTTP deployment must not receive HSTS even when opted in"
        );
    }

    /// The baseline headers are always present.
    #[test]
    fn baseline_headers_are_always_sent() {
        let headers = headers(false);
        assert_eq!(headers.get("x-content-type-options").unwrap(), "nosniff");
        assert_eq!(headers.get("x-frame-options").unwrap(), "DENY");
        assert_eq!(headers.get("referrer-policy").unwrap(), "same-origin");
        let csp = headers.get("content-security-policy").unwrap();
        let csp = csp.to_str().unwrap();
        assert!(csp.contains("default-src 'self'"));
        // Same-origin sockets stay allowed through `'self'`, but an
        // attacker-chosen `wss://` endpoint must not be.
        assert!(
            !csp.contains("ws:") && !csp.contains("wss:"),
            "connect-src must not allow arbitrary websocket origins: {csp}"
        );
        assert_eq!(
            headers.get("cross-origin-resource-policy").unwrap(),
            "same-origin"
        );
        // Inline scripts must stay rejected: every inline <script> was moved to
        // an external bundle or a JSON data block.
        let script_src = csp
            .split(';')
            .find(|d| d.trim_start().starts_with("script-src"))
            .expect("script-src directive");
        assert_eq!(script_src.trim(), "script-src 'self'");
        assert!(!csp.contains("script-src 'self' 'unsafe-inline'"));
    }
}
