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

/// Longest raw fallback kept when the browser cannot be recognised.
const MAX_FALLBACK_LEN: usize = 48;

/// Turn a stored `User-Agent` into something a person can act on.
///
/// The point of the label is letting the owner decide "is this me?", so it has
/// to name the browser and the platform. Ordering matters and is the whole
/// subtlety of the parser: every Chromium derivative also claims `Chrome/`, and
/// Chrome on iOS claims `Safari/`, so the most specific token is tested first.
///
/// An unrecognised agent falls back to a bounded prefix of the raw value, which
/// still distinguishes two rows from each other — better than printing nothing.
pub fn describe(raw: &str) -> String {
    let browser = if raw.contains("Edg/") || raw.contains("EdgA/") {
        "Edge"
    } else if raw.contains("OPR/") || raw.contains("Opera") {
        "Opera"
    } else if raw.contains("Firefox/") || raw.contains("FxiOS/") {
        "Firefox"
    } else if raw.contains("CriOS/") {
        // Chrome on iOS advertises `CriOS/` and `Safari/`, not `Chrome/`.
        "Chrome"
    } else if raw.contains("Chrome/") || raw.contains("Chromium/") {
        "Chrome"
    } else if raw.contains("Safari/") {
        "Safari"
    } else {
        return fallback(raw);
    };

    // iOS is checked before macOS: an iPhone's agent says "like Mac OS X".
    let platform = if raw.contains("Android") {
        "Android"
    } else if raw.contains("iPhone") || raw.contains("iPad") || raw.contains("iPod") {
        "iOS"
    } else if raw.contains("Windows") {
        "Windows"
    } else if raw.contains("Mac OS X") || raw.contains("Macintosh") {
        "macOS"
    } else if raw.contains("Linux") || raw.contains("X11") {
        "Linux"
    } else {
        return browser.to_string();
    };

    format!("{browser} · {platform}")
}

/// A bounded, single-line excerpt for an agent the parser does not know.
fn fallback(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.chars().count() <= MAX_FALLBACK_LEN {
        return trimmed.to_string();
    }
    let mut out: String = trimmed.chars().take(MAX_FALLBACK_LEN).collect();
    out.push('…');
    out
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

    #[test]
    fn real_agents_are_labelled() {
        for (raw, expected) in [
            (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
                "Chrome · Windows",
            ),
            (
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 \
                 (KHTML, like Gecko) Version/17.6 Safari/605.1.15",
                "Safari · macOS",
            ),
            (
                "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0",
                "Firefox · Linux",
            ),
            (
                "Mozilla/5.0 (iPhone; CPU iPhone OS 17_6 like Mac OS X) \
                 AppleWebKit/605.1.15 (KHTML, like Gecko) CriOS/128.0 Mobile/15E148 Safari/604.1",
                "Chrome · iOS",
            ),
            (
                "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/131.0 Mobile Safari/537.36",
                "Chrome · Android",
            ),
            (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/131.0 Safari/537.36 Edg/131.0",
                "Edge · Windows",
            ),
        ] {
            assert_eq!(describe(raw), expected, "raw: {raw}");
        }
    }

    /// A browser or platform the parser does not know must still produce
    /// something that tells two rows apart.
    #[test]
    fn unknown_agents_fall_back_to_a_bounded_excerpt() {
        assert_eq!(describe("curl/8.5.0"), "curl/8.5.0");
        assert_eq!(
            describe("NanofileTestBrowser/1.0"),
            "NanofileTestBrowser/1.0"
        );
        let long = "weird-agent/1.0 ".repeat(20);
        let described = describe(&long);
        assert!(described.ends_with('…'));
        assert_eq!(described.chars().count(), MAX_FALLBACK_LEN + 1);
        assert_eq!(describe("   "), "");
    }

    /// Chrome on iOS claims both `Safari/` and `Mac OS X`; the specific tokens
    /// have to win, or every iPhone shows up as desktop Safari.
    #[test]
    fn specific_tokens_win_over_the_generic_ones() {
        let ios_chrome = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_6 like Mac OS X) \
                          AppleWebKit/605.1.15 (KHTML, like Gecko) CriOS/128.0 Safari/604.1";
        assert_eq!(describe(ios_chrome), "Chrome · iOS");
    }
}
