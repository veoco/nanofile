//! Capture and presentation of the `User-Agent` a session was created with.
//!
//! A browser session has no `device_name`, so the raw `User-Agent` (bounded and
//! stripped of control characters) is what identifies a row to its owner; storing
//! it raw lets a better parser improve existing rows too.

use axum::http::HeaderMap;

/// Longest `User-Agent` kept; the bound stops a client from bloating every session row.
const MAX_USER_AGENT_LEN: usize = 512;

/// Read the request's `User-Agent`, bounded and stripped of control characters;
/// `None` when the header is missing, invalid UTF-8, or empty after cleaning.
pub fn from_headers(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(axum::http::header::USER_AGENT)?.to_str().ok()?;
    sanitize(raw)
}

/// Bound the value and remove control characters, which would corrupt the log
/// line and the layout. Askama escapes the rendered value, so this is defence in
/// depth rather than the only guard.
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

/// Turn a stored `User-Agent` into a label naming the browser and platform.
///
/// Ordering is the whole subtlety: every Chromium derivative also claims
/// `Chrome/`, and Chrome on iOS claims `Safari/`, so the most specific token is
/// tested first; an unknown agent falls back to a bounded raw prefix.
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

    /// An unknown agent must still produce something that tells two rows apart.
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

    /// Chrome on iOS claims both `Safari/` and `Mac OS X`; the specific tokens must win.
    #[test]
    fn specific_tokens_win_over_the_generic_ones() {
        let ios_chrome = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_6 like Mac OS X) \
                          AppleWebKit/605.1.15 (KHTML, like Gecko) CriOS/128.0 Safari/604.1";
        assert_eq!(describe(ios_chrome), "Chrome · iOS");
    }
}
