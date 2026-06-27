//! Embedded static assets service.
//!
//! Uses `rust-embed` to compile `static/` into the binary at build time.
//! Provides:
//! - Compile-time embedding of CSS, JS, favicon
//! - Lazy-computed SHA-256 content hashes for cache-busting filename fingerprints
//! - ETag, Last-Modified, Cache-Control: immutable headers
//! - Conditional request handling (304 Not Modified)
//! - `TemplateUrls` for Askama template use

use axum::{
    body::Body,
    extract::Path,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use rust_embed::Embed;
use std::sync::LazyLock;

// ─── Embed ───────────────────────────────────────────────────────────────────

#[derive(Embed)]
#[folder = "static/"]
struct Assets;

// ─── Hash helpers (SHA-256 prefix, once, cached) ────────────────────────────

fn hex_prefix(bytes: &[u8; 32]) -> String {
    let mut s = String::with_capacity(8);
    for &b in &bytes[..4] {
        s.push(std::char::from_digit((b >> 4) as u32, 16).unwrap());
        s.push(std::char::from_digit((b & 0x0f) as u32, 16).unwrap());
    }
    s
}

fn hex_long(bytes: &[u8; 32]) -> String {
    let mut s = String::with_capacity(16);
    for &b in &bytes[..8] {
        s.push(std::char::from_digit((b >> 4) as u32, 16).unwrap());
        s.push(std::char::from_digit((b & 0x0f) as u32, 16).unwrap());
    }
    s
}

fn http_date(unix_secs: u64) -> String {
    let ts = unix_secs as i64;
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|dt| dt.format("%a, %d %b %Y %H:%M:%S GMT").to_string())
        .unwrap_or_default()
}

// ─── Per-asset data (lazy, computed once from embedded file metadata) ───────

macro_rules! asset_data {
    ($hash:ident, $lm:ident, $etag:ident, $fp:ident, $path:expr) => {
        static $hash: LazyLock<String> = LazyLock::new(|| {
            let file =
                Assets::get($path).unwrap_or_else(|| panic!("embedded asset not found: {}", $path));
            hex_prefix(&file.metadata.sha256_hash())
        });

        static $lm: LazyLock<String> = LazyLock::new(|| {
            let file =
                Assets::get($path).unwrap_or_else(|| panic!("embedded asset not found: {}", $path));
            http_date(file.metadata.last_modified().unwrap_or(0))
        });

        static $etag: LazyLock<String> = LazyLock::new(|| {
            let file =
                Assets::get($path).unwrap_or_else(|| panic!("embedded asset not found: {}", $path));
            format!("\"{}\"", hex_long(&file.metadata.sha256_hash()))
        });

        /// Fingerprinted filename, e.g. `"css/app.abc123ef.css"`
        static $fp: LazyLock<String> = LazyLock::new(|| {
            let stem = $path.rsplit_once('.').map(|(s, _)| s).unwrap_or($path);
            let ext = $path.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
            format!("{}.{}.{}", stem, *$hash, ext)
        });
    };
}

asset_data!(CSS_HASH, CSS_LM, ETAG_CSS, FP_CSS, "css/app.css");
asset_data!(JS_HASH, JS_LM, ETAG_JS, FP_JS, "js/main.js");
asset_data!(
    FAVICON_HASH,
    FAVICON_LM,
    ETAG_FAVICON,
    FP_FAVICON,
    "img/favicon.svg"
);
asset_data!(
    FB_JS_HASH,
    FB_JS_LM,
    ETAG_FB_JS,
    FP_FB_JS,
    "js/file-browser.js"
);

// ─── Template URLs (for Askama templates, fingerprinted filenames) ──────────

/// Pre-computed static asset URLs with content-hash fingerprint in the filename.
///
/// Each field contains a full URL like `"/static/css/app.abc123ef.css"`.
/// References are `'static` — the strings live for the program's lifetime.
#[derive(Clone, Copy)]
pub struct TemplateUrls {
    pub css: &'static str,
    pub js: &'static str,
    pub file_browser_js: &'static str,
    pub favicon: &'static str,
    pub version: &'static str,
}

/// Return a reference to the lazily-initialised `TemplateUrls` singleton.
pub fn template_urls() -> &'static TemplateUrls {
    static URLS: LazyLock<TemplateUrls> = LazyLock::new(|| TemplateUrls {
        css: Box::leak(format!("/static/{}", *FP_CSS).into_boxed_str()),
        js: Box::leak(format!("/static/{}", *FP_JS).into_boxed_str()),
        file_browser_js: Box::leak(format!("/static/{}", *FP_FB_JS).into_boxed_str()),
        favicon: Box::leak(format!("/static/{}", *FP_FAVICON).into_boxed_str()),
        version: env!("CARGO_PKG_VERSION"),
    });
    &URLS
}

// ─── MIME type mapping ──────────────────────────────────────────────────────

fn mime_for(path: &str) -> &'static str {
    if let Some(ext) = path.rsplit('.').next() {
        match ext {
            "css" => "text/css; charset=utf-8",
            "js" => "application/javascript; charset=utf-8",
            "svg" => "image/svg+xml",
            _ => "application/octet-stream",
        }
    } else {
        "application/octet-stream"
    }
}

// ─── Asset resolution (fingerprinted path → original + metadata) ────────────

/// Resolve a request path (fingerprinted or plain) to its embedded original path
/// and associated ETag / Last-Modified strings.
fn resolve_asset(path: &str) -> Option<(&'static str, &'static str, &'static str)> {
    if path == "css/app.css" || path == *FP_CSS {
        Some(("css/app.css", &*ETAG_CSS, &*CSS_LM))
    } else if path == "js/main.js" || path == *FP_JS {
        Some(("js/main.js", &*ETAG_JS, &*JS_LM))
    } else if path == "js/file-browser.js" || path == *FP_FB_JS {
        Some(("js/file-browser.js", &*ETAG_FB_JS, &*FB_JS_LM))
    } else if path == "img/favicon.svg" || path == *FP_FAVICON {
        Some(("img/favicon.svg", &*ETAG_FAVICON, &*FAVICON_LM))
    } else {
        None
    }
}

// ─── Handler ────────────────────────────────────────────────────────────────

/// Serve an embedded static file with full HTTP caching semantics.
///
/// URLs use content-hash fingerprints in the filename:
/// `/static/css/app.abc123ef.css` — when content changes, the filename changes,
/// which naturally busts any cache without needing a query string.
///
/// Supports:
/// - `Cache-Control: public, max-age=31536000, immutable` (long-term caching)
/// - `ETag` / `Last-Modified` headers for validation
/// - Conditional `304 Not Modified` via `If-None-Match`
pub async fn serve_static(Path(path): Path<String>, headers: HeaderMap) -> Response {
    let path = path.trim_start_matches('/');

    let (original_path, etag, last_modified) = match resolve_asset(path) {
        Some(r) => r,
        None => return (StatusCode::NOT_FOUND, "404 Not Found").into_response(),
    };

    let Some(file) = Assets::get(original_path) else {
        return (StatusCode::NOT_FOUND, "404 Not Found").into_response();
    };

    // Conditional request: If-None-Match → 304
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        == Some(etag)
    {
        return (
            StatusCode::NOT_MODIFIED,
            [(header::CACHE_CONTROL, "public, max-age=31536000, immutable")],
        )
            .into_response();
    }

    // Normal response
    let mut resp = Response::builder()
        .header(header::CONTENT_TYPE, mime_for(original_path))
        .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
        .header(header::ETAG, etag);

    if !last_modified.is_empty() {
        resp = resp.header(header::LAST_MODIFIED, last_modified);
    }

    resp.body(Body::from(file.data))
        .unwrap_or_else(|_| (StatusCode::INTERNAL_SERVER_ERROR, "500").into_response())
}
