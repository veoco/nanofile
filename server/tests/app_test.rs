//! The assembled application.
//!
//! `tests/common` builds the server through `app::build_app`, the same call the
//! binary makes, so these assertions cover the whole assembly rather than one
//! handler: the routes every client needs, the middleware that has to run on
//! every response, and the two halves of the body-limit rule.
//!
//! They are also the guard on that sharing. If the harness ever goes back to
//! assembling its own router, the security headers and the configured body
//! limits go with it — and the two tests that assert them fail, rather than the
//! whole suite quietly drifting away from what the binary serves.

mod common;

use common::TestFixture;

#[tokio::test]
async fn the_readiness_endpoint_answers() {
    let f = TestFixture::new().await;
    let resp = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("{}/health", f.server.base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn every_response_carries_the_security_headers() {
    let f = TestFixture::new().await;
    let resp = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("{}/accounts/login/", f.server.base_url))
        .send()
        .await
        .unwrap();

    let headers = resp.headers();
    let csp = headers
        .get("content-security-policy")
        .expect("a page must carry a CSP")
        .to_str()
        .unwrap();
    // The two that are load-bearing for the pages as written: no inline script
    // is allowed anywhere, and nothing may be loaded off-origin.
    assert!(csp.contains("script-src 'self'"), "{csp}");
    assert!(!csp.contains("script-src 'self' 'unsafe-inline'"), "{csp}");
    assert_eq!(headers.get("x-content-type-options").unwrap(), "nosniff");
    assert_eq!(headers.get("x-frame-options").unwrap(), "DENY");
    assert_eq!(headers.get("referrer-policy").unwrap(), "same-origin");
}

#[tokio::test]
async fn a_body_over_the_configured_json_limit_is_refused() {
    let f = TestFixture::new().await;
    // The default JSON cap is 64 MiB; send more than that to a route that takes
    // JSON. The point is not the number but that the limit comes from the
    // config: a route with its own, smaller cap is asserted in the handler
    // tests, and this one would be unbounded if the global layer were missing.
    let body = vec![b'x'; 65 * 1024 * 1024];
    let resp = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!("{}/api2/repos/", f.server.base_url))
        .header("Authorization", format!("Token {}", f.api_token))
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        413,
        "a body past the configured JSON limit must be refused before it is buffered"
    );
}

#[tokio::test]
async fn an_upload_route_still_takes_a_large_body() {
    let f = TestFixture::new().await;
    // The same 65 MiB the JSON route just refused, sent to an upload route:
    // that group raised its own limit, so the global cap must not apply to it.
    // Without the per-group limit in `app_routes` this is the assertion that
    // fails.
    let body = vec![b'x'; 65 * 1024 * 1024];
    let resp = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!("{}/upload-aj/", f.server.base_url))
        .header("Authorization", format!("Token {}", f.api_token))
        .body(body)
        .send()
        .await
        .unwrap();

    assert_ne!(
        resp.status(),
        413,
        "an upload route must not inherit the JSON cap"
    );
}
