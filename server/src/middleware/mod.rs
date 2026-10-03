//! Axum middleware and extractors — authentication, permission checking.

pub mod auth;
pub mod repo_extractor;
pub mod route_audit;

use std::net::SocketAddr;

use crate::AppState;
use base::error::AppError;

use axum::extract::State;
use axum::http::{HeaderValue, header};

/// Add baseline security headers to every response.
///
/// `script-src 'self'` — no `'unsafe-inline'`: the pre-paint preference guards
/// load as a blocking external script, the share pages share one bundle, and
/// the translation table travels as a `type="application/json"` data block,
/// which is not executable and therefore not subject to `script-src`.
///
/// `style-src` still allows `'unsafe-inline'` for one reason: tag colours. A tag
/// carries an arbitrary hex value from the API (`_tag_color`), and `_tag_color`
/// reaches the page as a `style="background-color: …"` attribute because no
/// class can express a value the server learns at request time. Everything else
/// that used to need the directive is gone — the public pages' `<style>` blocks,
/// the modal's inline box-shadow, the progress-bar and left-panel width
/// attributes, and `base.html`'s first-paint background rule (now set from the
/// theme script). Tightening the directive therefore means restricting tags to a
/// fixed palette, which would break Seafile clients that write arbitrary
/// colours.
///
/// Everything else is locked to `'self'` (no fallback to `*`), so remote
/// script, frame, font and object loading is blocked.
///
/// `Strict-Transport-Security` is only sent when `site_url` is HTTPS: a
/// plain-HTTP LAN deployment must not be pinned to HTTPS by its own server.
/// Count a request as in flight for as long as it is being served.
///
/// The counter is what the load gauge reports as *foreground* load, so it is
/// the signal that decides whether a background job may run. The guard is RAII
/// and drops on every path, including a panic, so a failed request cannot make
/// the server look permanently busy.
pub async fn load_guard(
    State(state): State<std::sync::Arc<AppState>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let _guard = state.tasks.load().request_guard();
    next.run(request).await
}

pub async fn security_headers(
    axum::extract::State(state): axum::extract::State<std::sync::Arc<crate::AppState>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let mut response = next.run(req).await;
    apply_security_headers(
        response.headers_mut(),
        state.config().server.secure_cookies(),
        state.config().server.hsts_include_subdomains,
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
    if state.config().server.share_link_enabled {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

/// Determine the effective client IP for rate limiting.
///
/// Uses the TCP peer address exposed via `ConnectInfo`. `X-Forwarded-For` and
/// `CF-Connecting-IP` are only consulted when the peer is a configured
/// `trusted_proxies` entry (see [`proxy_matches`]); when the peer is not
/// trusted, those headers are ignored so a client cannot choose its own rate
/// limit bucket.
///
/// Behind a trusted proxy the client address is taken in this order:
///
/// 1. `CF-Connecting-IP` — Cloudflare writes this single, immutable client
///    address on every request from its edge to the origin (it overwrites any
///    client-supplied value), so it is the authoritative source when a
///    Cloudflare Tunnel / `cloudflared` sits in front of the server.
/// 2. `X-Forwarded-For` — walked **right to left**, skipping entries that are
///    trusted proxies, and the first untrusted hop is returned. This is the
///    ordinary reverse-proxy path (nginx/caddy), where the proxy appends to the
///    chain. Cloudflare additionally appends its *own* edge IP as the rightmost
///    entry, which is why the `CF-Connecting-IP` step above is preferred: it
///    avoids mistaking Cloudflare's edge for the visitor. Taking the leftmost
///    entry instead would hand the value to the client, since a forged
///    `X-Forwarded-For: 1.2.3.4` would sit left of the real client address.
///
/// If neither header yields a usable address, the peer itself is returned rather
/// than an attacker-supplied value.
pub fn effective_client_ip(
    addr: &SocketAddr,
    headers: &axum::http::HeaderMap,
    trusted_proxies: &[String],
) -> String {
    let peer = addr.ip();
    if !proxy_matches(&peer, trusted_proxies) {
        return peer.to_string();
    }

    // Cloudflare Tunnel / `cloudflared`: the edge sets a single, spoof-proof
    // client address. Prefer it over the X-Forwarded-For chain.
    if let Some(ip) = cf_connecting_ip(headers) {
        return ip;
    }

    if let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
        for hop in xff.split(',').rev() {
            let hop = hop.trim();
            if hop.is_empty() {
                continue;
            }
            if let Ok(ip) = hop.parse::<std::net::IpAddr>()
                && !proxy_matches(&ip, trusted_proxies)
            {
                return ip.to_string();
            }
        }
    }

    // Every hop was one of our own proxies (or the headers were absent/empty):
    // fall back to the peer rather than to an attacker-supplied value.
    peer.to_string()
}

/// Whether `peer` is a configured trusted reverse proxy.
///
/// Each entry is matched as one of:
/// - `loopback` / `private` / `link-local` / `unique-local` keywords,
/// - a CIDR range (e.g. `172.16.0.0/12`, `fd00::/8`), or
/// - an exact IP address (IPv4/IPv6).
///
/// Empty `trusted_proxies` means nothing sits in front of the server.
fn proxy_matches(peer: &std::net::IpAddr, entries: &[String]) -> bool {
    entries.iter().any(|e| entry_matches(peer, e.trim()))
}

fn entry_matches(peer: &std::net::IpAddr, entry: &str) -> bool {
    match entry.to_ascii_lowercase().as_str() {
        "loopback" => peer.is_loopback(),
        "private" => is_private(peer),
        "link-local" => is_link_local(peer),
        "unique-local" => is_unique_local(peer),
        _ => {
            if let Ok(net) = entry.parse::<ipnet::IpNet>() {
                net.contains(peer)
            } else if let Ok(ip) = entry.parse::<std::net::IpAddr>() {
                &ip == peer
            } else {
                false
            }
        }
    }
}

/// RFC1918 / loopback / link-local, plus the IPv6 equivalents.
fn is_private(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => v4.is_private() || v4.is_loopback() || v4.is_link_local(),
        std::net::IpAddr::V6(v6) => {
            v6.is_unique_local() || v6.is_loopback() || v6.is_unicast_link_local()
        }
    }
}

fn is_unique_local(ip: &std::net::IpAddr) -> bool {
    matches!(ip, std::net::IpAddr::V6(v6) if v6.is_unique_local())
}

fn is_link_local(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => v4.is_link_local(),
        std::net::IpAddr::V6(v6) => v6.is_unicast_link_local(),
    }
}

/// Parse `CF-Connecting-IP` into a single, validated client address.
///
/// Returns `None` when the header is absent or does not hold a single IP (so the
/// caller falls back to `X-Forwarded-For` / the peer rather than trusting a
/// malformed value).
fn cf_connecting_ip(headers: &axum::http::HeaderMap) -> Option<String> {
    let raw = headers
        .get("cf-connecting-ip")
        .and_then(|v| v.to_str().ok())?;
    let ip = raw.trim().parse::<std::net::IpAddr>().ok()?;
    Some(ip.to_string())
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

#[cfg(test)]
mod ip_tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderName, HeaderValue};

    fn sock(ip: &str) -> SocketAddr {
        let ip: std::net::IpAddr = ip.parse().unwrap();
        SocketAddr::new(ip, 1234)
    }

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    // An untrusted peer keeps the peer address regardless of any spoofed header.
    #[test]
    fn untrusted_peer_ignores_forwarded_headers() {
        let addr = sock("203.0.113.7");
        let hdrs = headers(&[
            ("cf-connecting-ip", "198.51.100.9"),
            ("x-forwarded-for", "198.51.100.9, 203.0.113.7"),
        ]);
        assert_eq!(
            effective_client_ip(&addr, &hdrs, &[]),
            "203.0.113.7",
            "an untrusted peer must not honour client-supplied headers"
        );
    }

    // The cloudflared scenario: peer is loopback (or a private bridge gateway)
    // and Cloudflare's edge IP header carries the real visitor.
    #[test]
    fn trusted_peer_prefers_cf_connecting_ip() {
        let addr = sock("127.0.0.1");
        let hdrs = headers(&[("cf-connecting-ip", "203.0.113.4")]);
        assert_eq!(
            effective_client_ip(&addr, &hdrs, &["private".into()]),
            "203.0.113.4"
        );
    }

    #[test]
    fn cf_connecting_ip_wins_over_xff() {
        let addr = sock("127.0.0.1");
        let hdrs = headers(&[
            ("cf-connecting-ip", "203.0.113.4"),
            // rightmost is Cloudflare's edge — the old code would have picked this
            ("x-forwarded-for", "203.0.113.4, 162.158.1.1"),
        ]);
        assert_eq!(
            effective_client_ip(&addr, &hdrs, &["private".into()]),
            "203.0.113.4"
        );
    }

    #[test]
    fn trusted_peer_without_cf_falls_back_to_xff() {
        let addr = sock("10.0.0.2");
        // ordinary reverse proxy: single client hop, no CF header
        let hdrs = headers(&[("x-forwarded-for", "203.0.113.9")]);
        assert_eq!(
            effective_client_ip(&addr, &hdrs, &["10.0.0.2".into()]),
            "203.0.113.9"
        );
    }

    #[test]
    fn trusted_by_cidr() {
        let addr = sock("172.17.0.1");
        let hdrs = headers(&[("cf-connecting-ip", "203.0.113.4")]);
        assert_eq!(
            effective_client_ip(&addr, &hdrs, &["172.16.0.0/12".into()]),
            "203.0.113.4"
        );
        // A peer just outside the range is not trusted.
        let outside = sock("172.32.0.1");
        assert_eq!(
            effective_client_ip(&outside, &hdrs, &["172.16.0.0/12".into()]),
            "172.32.0.1"
        );
    }

    #[test]
    fn trusted_by_loopback_keyword() {
        let addr = sock("::1");
        let hdrs = headers(&[("cf-connecting-ip", "203.0.113.4")]);
        assert_eq!(
            effective_client_ip(&addr, &hdrs, &["loopback".into()]),
            "203.0.113.4"
        );
    }

    #[test]
    fn malformed_cf_connecting_ip_falls_back() {
        let addr = sock("127.0.0.1");
        let hdrs = headers(&[
            ("cf-connecting-ip", "not-an-ip"),
            ("x-forwarded-for", "203.0.113.9"),
        ]);
        assert_eq!(
            effective_client_ip(&addr, &hdrs, &["private".into()]),
            "203.0.113.9"
        );
    }
}
