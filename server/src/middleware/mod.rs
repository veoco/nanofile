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
/// `script-src 'self'` allows no `'unsafe-inline'`: pending guards load as external
/// scripts and the translation table as a non-executable JSON block. `style-src`
/// still allows `'unsafe-inline'` only for tag colours, whose arbitrary API hex
/// reaches the page as a `style="…"` attribute no class could express; tightening
/// it would break Seafile clients. Everything else is locked to `'self'` (no `*`
/// fallback), so remote script, frame, font and object loading is blocked.
/// `Strict-Transport-Security` is sent only for an HTTPS `site_url`.
///
/// Count a request as in flight for as long as it is being served: the counter is
/// the *foreground* load the gauge reports, so it decides whether a background job
/// may run. The RAII guard drops on every path, including a panic.
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
/// `secure` means `site_url` is HTTPS; `hsts_include_subdomains` is opt-in since
/// sibling host names may serve unrelated plain-HTTP sites. `pub(crate)` for the
/// first-run setup listener ([`crate::setup`]), an `AppState`-less second router
/// that serves no TLS of its own and so passes `false` twice.
pub(crate) fn apply_security_headers(
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
    // Nothing here is meant to be embedded by another origin (X-Frame-Options and
    // `frame-ancestors` cover framing; this also blocks subresource embedding).
    headers.insert(
        header::HeaderName::from_static("cross-origin-resource-policy"),
        HeaderValue::from_static("same-origin"),
    );
    // A user-uploaded SVG is the one format handed to the browser as a *document*:
    // navigated to directly, it is parsed and runs in this origin, so a `<script>`
    // in it would be same-origin code. `sandbox` moves it to an opaque origin — no
    // scripts, same-origin reads, forms or top-level navigation. Keyed off the
    // response's own content type so every path serving an SVG inherits it; the
    // page policy is deliberately not sent, as nothing in an upload is a document
    // of this application.
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim);
    let uploaded_svg =
        content_type.is_some_and(|value| value.eq_ignore_ascii_case("image/svg+xml"));
    let is_html = content_type.is_some_and(|value| value.eq_ignore_ascii_case("text/html"));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(if uploaded_svg {
            "sandbox"
        } else {
            BASELINE_CSP
        }),
    );
    // Authenticated HTML carries CSRF tokens, file names and, on the 2FA page,
    // the TOTP secret. A shared cache must never store it; `no-store` also keeps
    // a password-protected share page out of a reverse proxy's cache.
    if is_html {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    }
    if secure {
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

/// The policy the application's own pages are written against.
const BASELINE_CSP: &str = "default-src 'self'; script-src 'self'; \
     style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; \
     font-src 'self'; connect-src 'self'; \
     object-src 'none'; frame-ancestors 'none'; base-uri 'self'; \
     form-action 'self'";

/// Enforce CSRF protection for a state-changing handler registered on `GET`.
///
/// [`AuthUser`](auth::AuthUser) skips its CSRF check for safe methods, because a
/// browser cannot attach a custom header to an `<img>` or `<link>` subresource. A
/// handler that mutates state while living on `GET` must therefore demand the
/// token itself, otherwise a cross-site subresource request carrying the victim's
/// session cookie is enough to perform the write. Bearer-token callers are left
/// alone: a cross-site request cannot set an `Authorization` header.
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

/// Reject a request when the download half of sharing is disabled by
/// `server.share_download_enabled`. Call at the top of handlers that create or
/// serve share links (`/f/`, `/d/`).
pub fn ensure_share_download_enabled(state: &std::sync::Arc<AppState>) -> Result<(), AppError> {
    if state.config().server.share_download_enabled {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

/// Reject a request when the upload half of sharing is disabled by
/// `server.share_upload_enabled`. Call at the top of handlers that create or serve
/// upload links (`/u/`), and at every place an anonymous upload token is consumed.
pub fn ensure_share_upload_enabled(state: &std::sync::Arc<AppState>) -> Result<(), AppError> {
    if state.config().server.share_upload_enabled {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

/// Determine the effective client IP for rate limiting.
///
/// The TCP peer address is authoritative unless it is a configured
/// `trusted_proxies` entry; otherwise forwarded headers are ignored so a client
/// cannot choose its own rate-limit bucket. Behind a trusted proxy,
/// `CF-Connecting-IP` wins: Cloudflare overwrites it with the single client
/// address, so it avoids mistaking the edge for the visitor. `X-Forwarded-For` is
/// walked **right to left**, skipping trusted proxies, because a forged entry sits
/// to the left of the real client. If no header yields a usable address, the peer
/// is returned rather than an attacker-supplied value.
pub fn effective_client_ip(
    addr: &SocketAddr,
    headers: &axum::http::HeaderMap,
    trusted_proxies: &[String],
) -> String {
    let peer = addr.ip();
    if !proxy_matches(&peer, trusted_proxies) {
        return peer.to_string();
    }

    // Cloudflare sets a single, spoof-proof client address; prefer it over XFF.
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

    // Every hop was ours (or headers absent): fall back to the peer, not a header.
    peer.to_string()
}

/// Whether `peer` is a configured trusted reverse proxy: an entry may be a
/// `loopback`/`private`/`link-local`/`unique-local` keyword, a CIDR range (e.g.
/// `172.16.0.0/12`, `fd00::/8`) or an exact IP. Empty means nothing sits in front
/// of the server.
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

/// Parse `CF-Connecting-IP` into a single validated address, `None` when absent or
/// malformed so the caller falls back rather than trusting it.
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

    /// HSTS is only meaningful over HTTPS: a plain-HTTP deployment cannot serve it.
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
        // Same-origin sockets stay allowed through `'self'`, not arbitrary wss://.
        assert!(
            !csp.contains("ws:") && !csp.contains("wss:"),
            "connect-src must not allow arbitrary websocket origins: {csp}"
        );
        assert_eq!(
            headers.get("cross-origin-resource-policy").unwrap(),
            "same-origin"
        );
        // No inline <script> remains: bundles and JSON blocks only.
        let script_src = csp
            .split(';')
            .find(|d| d.trim_start().starts_with("script-src"))
            .expect("script-src directive");
        assert_eq!(script_src.trim(), "script-src 'self'");
        assert!(!csp.contains("script-src 'self' 'unsafe-inline'"));
    }

    /// A user-uploaded SVG is parsed as a document when navigated to directly, so
    /// `sandbox` keeps it out of this origin; the page policy would allow scripts.
    #[test]
    fn an_uploaded_svg_is_sandboxed() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("image/svg+xml"),
        );
        apply_security_headers(&mut headers, false, false);
        assert_eq!(headers.get("content-security-policy").unwrap(), "sandbox");
        assert_eq!(headers.get("x-content-type-options").unwrap(), "nosniff");
    }

    /// Pages that carry per-user state (CSRF tokens, file names, TOTP secrets)
    /// are never stored by a browser or shared cache. Static assets keep their
    /// long cache: the header is keyed off the response's own content type.
    #[test]
    fn html_is_not_cacheable_but_assets_are() {
        let mut html = axum::http::HeaderMap::new();
        html.insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("text/html; charset=utf-8"),
        );
        apply_security_headers(&mut html, false, false);
        assert_eq!(html.get("cache-control").unwrap(), "no-store");
        assert_eq!(html.get("pragma").unwrap(), "no-cache");

        let mut css = axum::http::HeaderMap::new();
        css.insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("text/css"),
        );
        apply_security_headers(&mut css, false, false);
        assert!(
            css.get("cache-control").is_none(),
            "static CSS must keep its long cache"
        );
    }

    /// Everything else keeps the baseline policy, even a near-miss Content-Type.
    #[test]
    fn a_non_svg_response_keeps_the_baseline_policy() {
        for content_type in [
            "text/html; charset=utf-8",
            "image/png",
            "image/svg+xml-compressed",
            "application/pdf",
        ] {
            let mut headers = axum::http::HeaderMap::new();
            headers.insert(
                axum::http::header::CONTENT_TYPE,
                axum::http::HeaderValue::from_str(content_type).unwrap(),
            );
            apply_security_headers(&mut headers, false, false);
            let csp = headers
                .get("content-security-policy")
                .unwrap()
                .to_str()
                .unwrap();
            assert!(
                csp.contains("default-src 'self'"),
                "{content_type} lost the baseline policy: {csp}"
            );
        }
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

    // The cloudflared scenario: loopback peer, real visitor in the CF header.
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
