//! Axum middleware and extractors — authentication, permission checking.

pub mod auth;
pub mod repo_extractor;

use std::net::SocketAddr;

use crate::AppState;
use base::error::AppError;

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
