//! Cookie header parsing shared by authentication and CSRF checks.
//!
//! One parser for every caller: the Web UI extractor and the CSRF helper each
//! scanned the header themselves before, so a request whose session cookie sat in
//! a second `Cookie` header authenticated on one surface but skipped the CSRF
//! check on another.

use axum::http::HeaderMap;
use axum_extra::headers::{Cookie, Header};

/// The value of one cookie from a request's `Cookie` headers.
///
/// `Cookie::decode` is the primary path because it knows the header rules (quoted
/// values, several `Cookie` headers), but it rejects the whole header on a segment
/// it cannot parse 鈥?and a browser will happily send one. The plain scan stays as
/// a fallback so a malformed neighbour does not turn a working session into a 401.
pub fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    if let Ok(cookie) = Cookie::decode(&mut headers.get_all("cookie").iter())
        && let Some(value) = cookie.get(name)
    {
        return Some(value.to_string());
    }
    headers
        .get_all("cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|header| header.split(';'))
        .map(str::trim)
        .find_map(|pair| {
            pair.strip_prefix(name)
                .and_then(|rest| rest.strip_prefix('='))
                .map(str::to_string)
        })
}

/// The `seahub-session` cookie's value, if the request carries one.
pub fn session_cookie_value(headers: &HeaderMap) -> Option<String> {
    cookie_value(headers, "seahub-session")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    /// The parser finds the session cookie in the header shapes a browser actually
    /// sends, and still finds it next to a segment `Cookie::decode` rejects.
    #[test]
    fn the_session_cookie_is_found_whatever_the_header_shape() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "cookie",
            HeaderValue::from_static("seahub-session=first; sfcsrftoken=x"),
        );
        assert_eq!(
            cookie_value(&headers, "seahub-session").as_deref(),
            Some("first")
        );

        // A segment without `=` makes the strict decoder reject the whole header; the scan still
        // finds the session cookie, so a malformed neighbour does not log the browser out.
        headers.insert(
            "cookie",
            HeaderValue::from_static("garbage; seahub-session=plain"),
        );
        assert_eq!(
            cookie_value(&headers, "seahub-session").as_deref(),
            Some("plain")
        );

        // Split across two headers, as a request that accumulated cookies can be.
        let mut headers = HeaderMap::new();
        headers.append("cookie", HeaderValue::from_static("a=1"));
        headers.append("cookie", HeaderValue::from_static("seahub-session=second"));
        assert_eq!(
            cookie_value(&headers, "seahub-session").as_deref(),
            Some("second")
        );

        // A cookie whose name merely ends with the one asked for is not a match.
        let mut headers = HeaderMap::new();
        headers.insert("cookie", HeaderValue::from_static("xseahub-session=nope"));
        assert_eq!(cookie_value(&headers, "seahub-session"), None);

        let mut headers = HeaderMap::new();
        headers.insert("cookie", HeaderValue::from_static("other=1"));
        assert_eq!(cookie_value(&headers, "seahub-session"), None);
    }
}
