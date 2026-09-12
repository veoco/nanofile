//! Capture and presentation of the `User-Agent` a session was created with.
//!
//! A browser session has no `device_name` to identify it, so the login's
//! `User-Agent` is the only thing that tells the owner *which* session a row in
//! the inventory is. It is stored raw (bounded and stripped of control
//! characters) rather than as a parsed label, so improving the parser later
//! improves every existing row instead of only new ones.

use axum::http::HeaderMap;

/// Longest `User-Agent` kept. Real ones are well under 200 characters; the
/// bound stops a client from turning every session row into a large blob.
const MAX_USER_AGENT_LEN: usize = 512;

/// Read the request's `User-Agent`, bounded and stripped of control characters.
///
/// Returns `None` when the header is missing, not valid UTF-8, or empty after
/// cleaning — all of which mean "nothing worth showing".
pub fn from_headers(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(axum::http::header::USER_AGENT)?.to_str().ok()?;
    sanitize(raw)
}

/// Bound the value and remove anything that could break a rendered page.
///
/// A stored label is echoed back into HTML by askama, which escapes it, so this
/// is defence in depth rather than the only guard: control characters would
/// still corrupt the log line and the layout.
pub fn sanitize(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_USER_AGENT_LEN)
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
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

    #[test]
    fn control_characters_and_blank_values_are_dropped() {
        assert_eq!(sanitize("  \u{7}  "), None);
        assert_eq!(sanitize("").as_deref(), None);
        assert_eq!(
            sanitize("Mozilla\u{0}/5.0\nInjected: yes").as_deref(),
            Some("Mozilla/5.0Injected: yes")
        );
    }

    #[test]
    fn an_oversized_value_is_bounded() {
        let long = "a".repeat(MAX_USER_AGENT_LEN * 2);
        assert_eq!(sanitize(&long).expect("ua").len(), MAX_USER_AGENT_LEN);
    }
}
