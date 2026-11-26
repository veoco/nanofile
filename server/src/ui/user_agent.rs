//! Capture of the `User-Agent` a session was created with.
//!
//! The pure rules (bounding, control-character stripping, browser/platform
//! labelling) live in `base::user_agent`; this module only knows how to read the
//! header off an HTTP request.

use axum::http::HeaderMap;

pub use base::user_agent::{describe, sanitize};

/// Read the request's `User-Agent`, bounded and stripped of control characters;
/// `None` when the header is missing, invalid UTF-8, or empty after cleaning.
pub fn from_headers(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(axum::http::header::USER_AGENT)?.to_str().ok()?;
    sanitize(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::header;

    fn headers(value: &str) -> HeaderMap {
        let mut map = HeaderMap::new();
        map.insert(header::USER_AGENT, value.parse().expect("header value"));
        map
    }

    #[test]
    fn reads_the_request_header() {
        let ua = from_headers(&headers("Mozilla/5.0 (Macintosh) Firefox/128.0")).expect("ua");
        assert!(ua.contains("Firefox/128.0"));
    }

    #[test]
    fn a_missing_header_is_not_an_error() {
        assert_eq!(from_headers(&HeaderMap::new()), None);
    }
}
