//! Axum middleware and extractors — authentication, permission checking.

pub mod auth;
pub mod repo_extractor;

use std::net::SocketAddr;

/// Determine the effective client IP for rate limiting.
///
/// Uses the TCP peer address exposed via `ConnectInfo`. The `X-Forwarded-For`
/// header is only honored when the TCP peer is in `trusted_proxies` (set when
/// the server runs behind a reverse proxy). Without this, attackers could spoof
/// `X-Forwarded-For` to bypass per-IP rate limits.
pub fn effective_client_ip(
    addr: &SocketAddr,
    headers: &axum::http::HeaderMap,
    trusted_proxies: &[String],
) -> String {
    let peer = addr.ip().to_string();
    if trusted_proxies.iter().any(|p| p == &peer)
        && let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok())
        && let Some(first) = xff.split(',').next().map(|s| s.trim())
        && !first.is_empty()
    {
        return first.to_string();
    }
    peer
}
