//! Embedded static assets service.
//!
//! Assets are compiled into the binary with `include-flate` (pure-Rust DEFLATE)
//! and served with content-hash fingerprints in the filename, `ETag` /
//! `Last-Modified` and `304 Not Modified` handling.

use axum::{
    body::Body,
    extract::Path,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::LazyLock;

// The files this service can serve. A path that is not listed here is a 404,
// even if it exists under `static/`: add it here (and to `ASSET_CACHE`) when a
// template starts referencing a new asset.
include_flate::flate!(static APP_CSS: [u8] from "static/css/app.css" with deflate);
include_flate::flate!(static COMMON_JS: [u8] from "static/js/common.bundle.js" with deflate);
include_flate::flate!(static FB_JS: [u8] from "static/js/file-browser.bundle.js" with deflate);
include_flate::flate!(static PUBLIC_UPLOAD_JS: [u8] from "static/js/public-upload.bundle.js" with deflate);
include_flate::flate!(static THEME_INIT_JS: [u8] from "static/js/theme-init.bundle.js" with deflate);
include_flate::flate!(static PUBLIC_SHARE_JS: [u8] from "static/js/public-share.bundle.js" with deflate);
include_flate::flate!(static POW_JS: [u8] from "static/js/pow.bundle.js" with deflate);
include_flate::flate!(static FAVICON: [u8] from "static/img/favicon.svg" with deflate);

/// What the serving code needs from an embedded file: the inflated bytes and the
/// metadata an `ETag` / `Last-Modified` is derived from.
struct EmbeddedFile {
    data: Cow<'static, [u8]>,
    metadata: Metadata,
}

#[derive(Clone, Copy)]
struct Metadata {
    sha256: [u8; 32],
    last_modified: u64,
}

impl Metadata {
    fn sha256_hash(&self) -> [u8; 32] {
        self.sha256
    }

    fn last_modified(&self) -> Option<u64> {
        Some(self.last_modified)
    }
}

struct CachedAsset {
    bytes: bytes::Bytes,
    sha256: [u8; 32],
    last_modified: u64,
}

impl CachedAsset {
    fn new(raw: &[u8], last_modified: u64) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(raw);
        let mut sha256 = [0u8; 32];
        sha256.copy_from_slice(&hasher.finalize());
        Self {
            bytes: bytes::Bytes::copy_from_slice(raw),
            sha256,
            last_modified,
        }
    }
}

/// Every asset is inflated and hashed once, on the first lookup, and the bytes
/// are shared from here on. The whole set is ~300 KiB, so the one-time cost is
/// small; inflating per request is what this avoids.
static ASSET_CACHE: LazyLock<HashMap<&'static str, CachedAsset>> = LazyLock::new(|| {
    // The assets are build products, so they all share the build timestamp; the
    // ETag, which is content-derived, is what browsers actually revalidate on.
    let last_modified: u64 = env!("NANOFILE_BUILD_TS").parse().unwrap_or(0);
    HashMap::from([
        (
            "css/app.css",
            CachedAsset::new(APP_CSS.as_slice(), last_modified),
        ),
        (
            "js/common.bundle.js",
            CachedAsset::new(COMMON_JS.as_slice(), last_modified),
        ),
        (
            "js/file-browser.bundle.js",
            CachedAsset::new(FB_JS.as_slice(), last_modified),
        ),
        (
            "js/public-upload.bundle.js",
            CachedAsset::new(PUBLIC_UPLOAD_JS.as_slice(), last_modified),
        ),
        (
            "js/theme-init.bundle.js",
            CachedAsset::new(THEME_INIT_JS.as_slice(), last_modified),
        ),
        (
            "js/public-share.bundle.js",
            CachedAsset::new(PUBLIC_SHARE_JS.as_slice(), last_modified),
        ),
        (
            "js/pow.bundle.js",
            CachedAsset::new(POW_JS.as_slice(), last_modified),
        ),
        (
            "img/favicon.svg",
            CachedAsset::new(FAVICON.as_slice(), last_modified),
        ),
    ])
});

/// Look up a path under `static/` and get its inflated bytes plus metadata.
struct Assets;

impl Assets {
    fn get(path: &str) -> Option<EmbeddedFile> {
        let cached = ASSET_CACHE.get(path)?;
        Some(EmbeddedFile {
            data: Cow::Borrowed(cached.bytes.as_ref()),
            metadata: Metadata {
                sha256: cached.sha256,
                last_modified: cached.last_modified,
            },
        })
    }
}

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

/// A shareable `Bytes` handle for one embedded asset.
///
/// `ASSET_CACHE` inflates each file once; this wrapper hands the response path a
/// cheap clone of the inflated bytes. The set is ~300 KiB, a fair trade for the
/// smaller binary and an allocation-free hot path.
macro_rules! asset_bytes {
    ($name:ident, $path:expr) => {
        static $name: LazyLock<bytes::Bytes> = LazyLock::new(|| {
            let file =
                Assets::get($path).unwrap_or_else(|| panic!("embedded asset not found: {}", $path));
            match file.data {
                Cow::Borrowed(data) => bytes::Bytes::from_static(data),
                Cow::Owned(data) => bytes::Bytes::from(data),
            }
        });
    };
}

asset_bytes!(CSS_BYTES, "css/app.css");
asset_bytes!(COMMON_JS_BYTES, "js/common.bundle.js");
asset_bytes!(FB_JS_BYTES, "js/file-browser.bundle.js");
asset_bytes!(PUBLIC_UPLOAD_JS_BYTES, "js/public-upload.bundle.js");
asset_bytes!(THEME_INIT_JS_BYTES, "js/theme-init.bundle.js");
asset_bytes!(PUBLIC_SHARE_JS_BYTES, "js/public-share.bundle.js");
asset_bytes!(POW_JS_BYTES, "js/pow.bundle.js");
asset_bytes!(FAVICON_BYTES, "img/favicon.svg");
asset_data!(
    COMMON_JS_HASH,
    COMMON_JS_LM,
    ETAG_COMMON_JS,
    FP_COMMON_JS,
    "js/common.bundle.js"
);
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
    "js/file-browser.bundle.js"
);
asset_data!(
    PUBLIC_UPLOAD_JS_HASH,
    PUBLIC_UPLOAD_JS_LM,
    ETAG_PUBLIC_UPLOAD_JS,
    FP_PUBLIC_UPLOAD_JS,
    "js/public-upload.bundle.js"
);
asset_data!(
    THEME_INIT_JS_HASH,
    THEME_INIT_JS_LM,
    ETAG_THEME_INIT_JS,
    FP_THEME_INIT_JS,
    "js/theme-init.bundle.js"
);
asset_data!(
    PUBLIC_SHARE_JS_HASH,
    PUBLIC_SHARE_JS_LM,
    ETAG_PUBLIC_SHARE_JS,
    FP_PUBLIC_SHARE_JS,
    "js/public-share.bundle.js"
);
asset_data!(
    POW_JS_HASH,
    POW_JS_LM,
    ETAG_POW_JS,
    FP_POW_JS,
    "js/pow.bundle.js"
);

/// Pre-computed static asset URLs with a content-hash fingerprint in the filename,
/// e.g. `"/static/css/app.abc123ef.css"`; the strings are `'static`.
#[derive(Clone, Copy)]
pub struct TemplateUrls {
    pub css: &'static str,
    pub common_js: &'static str,
    pub file_browser_js: &'static str,
    pub public_upload_js: &'static str,
    /// Blocking `<head>` script restoring the saved theme/view-mode before the
    /// first paint (kept out of the inline CSP).
    pub theme_init_js: &'static str,
    /// Timestamp formatting for the public share pages.
    pub public_share_js: &'static str,
    /// Browser solver for the anonymous form proof-of-work challenge.
    pub pow_js: &'static str,
    pub favicon: &'static str,
    pub version: &'static str,
}

/// Return a reference to the lazily-initialised `TemplateUrls` singleton.
pub fn template_urls() -> &'static TemplateUrls {
    static URLS: LazyLock<TemplateUrls> = LazyLock::new(|| TemplateUrls {
        css: Box::leak(format!("/static/{}", *FP_CSS).into_boxed_str()),
        common_js: Box::leak(format!("/static/{}", *FP_COMMON_JS).into_boxed_str()),
        file_browser_js: Box::leak(format!("/static/{}", *FP_FB_JS).into_boxed_str()),
        public_upload_js: Box::leak(format!("/static/{}", *FP_PUBLIC_UPLOAD_JS).into_boxed_str()),
        theme_init_js: Box::leak(format!("/static/{}", *FP_THEME_INIT_JS).into_boxed_str()),
        public_share_js: Box::leak(format!("/static/{}", *FP_PUBLIC_SHARE_JS).into_boxed_str()),
        pow_js: Box::leak(format!("/static/{}", *FP_POW_JS).into_boxed_str()),
        favicon: Box::leak(format!("/static/{}", *FP_FAVICON).into_boxed_str()),
        version: env!("CARGO_PKG_VERSION"),
    });
    &URLS
}

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

/// Resolve a request path (fingerprinted or plain) to its embedded original path,
/// associated ETag / Last-Modified strings, and the cached decompressed bytes.
fn resolve_asset(
    path: &str,
) -> Option<(
    &'static str,
    &'static str,
    &'static str,
    &'static bytes::Bytes,
)> {
    if path == "css/app.css" || path == *FP_CSS {
        Some(("css/app.css", &*ETAG_CSS, &*CSS_LM, &*CSS_BYTES))
    } else if path == "js/common.bundle.js" || path == *FP_COMMON_JS {
        Some((
            "js/common.bundle.js",
            &*ETAG_COMMON_JS,
            &*COMMON_JS_LM,
            &*COMMON_JS_BYTES,
        ))
    } else if path == "js/file-browser.bundle.js" || path == *FP_FB_JS {
        Some((
            "js/file-browser.bundle.js",
            &*ETAG_FB_JS,
            &*FB_JS_LM,
            &*FB_JS_BYTES,
        ))
    } else if path == "js/public-upload.bundle.js" || path == *FP_PUBLIC_UPLOAD_JS {
        Some((
            "js/public-upload.bundle.js",
            &*ETAG_PUBLIC_UPLOAD_JS,
            &*PUBLIC_UPLOAD_JS_LM,
            &*PUBLIC_UPLOAD_JS_BYTES,
        ))
    } else if path == "js/theme-init.bundle.js" || path == *FP_THEME_INIT_JS {
        Some((
            "js/theme-init.bundle.js",
            &*ETAG_THEME_INIT_JS,
            &*THEME_INIT_JS_LM,
            &*THEME_INIT_JS_BYTES,
        ))
    } else if path == "js/public-share.bundle.js" || path == *FP_PUBLIC_SHARE_JS {
        Some((
            "js/public-share.bundle.js",
            &*ETAG_PUBLIC_SHARE_JS,
            &*PUBLIC_SHARE_JS_LM,
            &*PUBLIC_SHARE_JS_BYTES,
        ))
    } else if path == "js/pow.bundle.js" || path == *FP_POW_JS {
        Some((
            "js/pow.bundle.js",
            &*ETAG_POW_JS,
            &*POW_JS_LM,
            &*POW_JS_BYTES,
        ))
    } else if path == "img/favicon.svg" || path == *FP_FAVICON {
        Some((
            "img/favicon.svg",
            &*ETAG_FAVICON,
            &*FAVICON_LM,
            &*FAVICON_BYTES,
        ))
    } else {
        None
    }
}

/// Serve an embedded static file with full HTTP caching semantics.
///
/// Fingerprinted URLs (`/static/css/app.abc123ef.css`) change when the content
/// does, busting caches without a query string. Responses carry
/// `Cache-Control: public, max-age=31536000, immutable`, `ETag` / `Last-Modified`,
/// and answer a matching `If-None-Match` with `304 Not Modified`.
pub async fn serve_static(Path(path): Path<String>, headers: HeaderMap) -> Response {
    let path = path.trim_start_matches('/');

    let (original_path, etag, last_modified, data) = match resolve_asset(path) {
        Some(r) => r,
        None => return (StatusCode::NOT_FOUND, "404 Not Found").into_response(),
    };

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

    let mut resp = Response::builder()
        .header(header::CONTENT_TYPE, mime_for(original_path))
        .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
        .header(header::ETAG, etag);

    if !last_modified.is_empty() {
        resp = resp.header(header::LAST_MODIFIED, last_modified);
    }

    resp.body(Body::from(data.clone()))
        .unwrap_or_else(|_| (StatusCode::INTERNAL_SERVER_ERROR, "500").into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// First value of an XML attribute `name="…"` in `src`.
    fn attr<'a>(src: &'a str, name: &str) -> &'a str {
        let needle = format!("{name}=\"");
        let start = src
            .find(&needle)
            .unwrap_or_else(|| panic!("no {name} attribute"))
            + needle.len();
        let end = start + src[start..].find('"').expect("unterminated attribute");
        &src[start..end]
    }

    /// `favicon.svg` (also what the tray/exe icons rasterize from) and
    /// `icon_brand_mark` are two copies of one geometry, so they drift silently:
    /// the mark once rendered at four different radii and four different glyph
    /// weights across the tab, the topbar, the rail footer and the brand bar.
    /// Compare the parts that carry the shape, not the parts that must differ
    /// (a standalone favicon cannot read the app's CSS variables for colour).
    #[test]
    fn favicon_and_in_app_mark_share_one_geometry() {
        let favicon_file = Assets::get("img/favicon.svg").expect("embedded favicon");
        let favicon = std::str::from_utf8(&favicon_file.data).unwrap();
        let icons = include_str!("../templates/includes/icons.html");
        let start = icons.find("macro icon_brand_mark").expect("brand macro");
        let mark = &icons[start..];
        let mark = &mark[..mark.find("endmacro").expect("macro end")];

        assert_eq!(attr(favicon, "viewBox"), attr(mark, "viewBox"));
        assert_eq!(attr(favicon, "rx"), attr(mark, "rx"));
        assert_eq!(attr(favicon, "d"), attr(mark, "d"));
    }

    /// The compressed embed is inflated once and served byte-for-byte; the ETag
    /// that makes the URL immutable must still match the uncompressed content.
    #[tokio::test]
    async fn serves_the_cached_decompressed_asset_and_revalidates() {
        let resp = serve_static(Path("css/app.css".to_string()), HeaderMap::new()).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let etag = resp
            .headers()
            .get(header::ETAG)
            .expect("ETag header")
            .to_str()
            .expect("ASCII ETag")
            .to_string();
        let body = axum::body::to_bytes(resp.into_body(), 8 * 1024 * 1024)
            .await
            .expect("read body");
        let expected = Assets::get("css/app.css").expect("embedded CSS");
        assert_eq!(&body[..], expected.data.as_ref());

        let mut headers = HeaderMap::new();
        headers.insert(
            header::IF_NONE_MATCH,
            etag.parse().expect("valid ETag header value"),
        );
        let resp = serve_static(Path("css/app.css".to_string()), headers).await;
        assert_eq!(resp.status(), StatusCode::NOT_MODIFIED);
    }
}
