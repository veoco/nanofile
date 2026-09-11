//! Axum middleware and extractors — authentication, permission checking.

pub mod auth;
pub mod repo_extractor;

use std::net::SocketAddr;

use crate::AppState;
use base::error::AppError;

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
