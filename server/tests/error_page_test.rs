//! The browser-facing error pages.
//!
//! `AppError` answers with the Seafile wire protocol, and the desktop client,
//! the CLI and the frontend's fetch calls depend on that. These tests pin the
//! other half of the rule: on a *page* route a browser gets a page, and every
//! wire endpoint keeps its JSON no matter what the browser asks for.

mod common;

use common::TestFixture;

/// What a browser sends.
const BROWSER: &str = "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8";

/// A cookie-holding client with redirects left visible, i.e. a browser session.
async fn login_client(f: &TestFixture) -> reqwest::Client {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .cookie_store(true)
        .build()
        .unwrap();
    let resp = client
        .post(format!("{}/accounts/login/", f.server.base_url))
        .form(&[("email", &f.email), ("password", &f.password)])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 302, "login for test helper");
    client
}

/// GET a page the way a browser would, and return the status and body.
async fn browse(f: &TestFixture, path: &str) -> (reqwest::StatusCode, String) {
    let resp = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("{}{}", f.server.base_url, path))
        .header("Accept", BROWSER)
        .send()
        .await
        .unwrap();
    let status = resp.status();
    (status, resp.text().await.unwrap())
}

/// The signed-in variant of [`browse`].
async fn browse_as(
    client: &reqwest::Client,
    f: &TestFixture,
    path: &str,
) -> (reqwest::StatusCode, String) {
    let resp = client
        .get(format!("{}{}", f.server.base_url, path))
        .header("Accept", BROWSER)
        .send()
        .await
        .unwrap();
    let status = resp.status();
    (status, resp.text().await.unwrap())
}

#[tokio::test]
async fn an_unknown_path_renders_a_page_for_a_browser() {
    let f = TestFixture::new().await;
    let (status, body) = browse(&f, "/no/such/page/").await;

    assert_eq!(status, 404);
    assert!(body.contains("Page not found"), "{body}");
    // A visitor with no session is pointed at the one thing they can do.
    assert!(body.contains(">Sign in<"), "{body}");
    // The wire body never reaches the browser.
    assert!(!body.contains("error_msg"), "{body}");
}

#[tokio::test]
async fn an_unknown_path_keeps_the_empty_body_for_a_client() {
    let f = TestFixture::new().await;
    let resp = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("{}/no/such/page/", f.server.base_url))
        // No Accept: what a script, a probe or a legacy client sends.
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 404);
    assert!(
        resp.headers().get("content-type").is_none(),
        "a client must keep the body-less 404 it has always received"
    );
    assert!(resp.text().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_dead_share_link_shows_the_link_page() {
    let f = TestFixture::new().await;
    let (status, body) = browse(&f, "/f/no-such-token-at-all/").await;

    assert_eq!(status, 404);
    assert!(body.contains("Link unavailable"), "{body}");
    // Said in the reader's terms: the code is the server's story, so it is not
    // what the page leads with.
    assert!(!body.contains("error_msg"), "{body}");
    // The reader has no account on this route, so there is nothing to go back
    // to beyond the site itself.
    assert!(body.contains("Go to Nanofile"), "{body}");
}

#[tokio::test]
async fn an_expired_share_link_says_so() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/",
                // Negative expiry: the link is born expired.
                "expire_days": -1,
            }),
        )
        .await;
    assert_eq!(resp.status(), 200, "create share link failed");
    let token = resp.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    let (status, body) = browse(&f, &format!("/d/{token}/")).await;
    assert_eq!(status, 404);
    // "Expired" is worth telling apart from "turned off": one of them the
    // reader can fix by asking for a new link, the other they cannot.
    assert!(body.contains("Link expired"), "{body}");
    assert!(!body.contains("Link unavailable"), "{body}");
}

#[tokio::test]
async fn a_signed_in_reader_gets_a_page_inside_the_app() {
    let f = TestFixture::new().await;
    let client = login_client(&f).await;

    // A library id that cannot exist.
    let path = "/libraries/00000000-0000-0000-0000-000000000000/files/";
    let (status, body) = browse_as(&client, &f, path).await;

    assert_eq!(status, 404);
    assert!(body.contains("Page not found"), "{body}");
    // The app chrome is still there, so the reader can navigate away.
    assert!(body.contains("Back to libraries"), "{body}");
    assert!(body.contains(r#"id="__i18n""#), "app shell missing: {body}");
    assert!(body.contains("/settings/"), "app shell missing: {body}");
    assert!(!body.contains("error_msg"), "{body}");
}

#[tokio::test]
async fn a_failed_request_is_not_cached() {
    let f = TestFixture::new().await;
    let resp = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("{}/no/such/page/", f.server.base_url))
        .header("Accept", BROWSER)
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.headers().get("cache-control").unwrap(),
        "no-store",
        "a transient failure must not be cached into a permanent one"
    );
}

/// The other half of the rule: which route, not which `Accept` header. A client
/// that happens to ask for HTML still gets the wire body from the API.
#[tokio::test]
async fn the_api_namespaces_keep_their_wire_bodies() {
    let f = TestFixture::new().await;
    for path in [
        "/api/v2.1/repos/00000000-0000-0000-0000-000000000000/",
        "/api2/repos/00000000-0000-0000-0000-000000000000/",
    ] {
        let resp = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(format!("{}{}", f.server.base_url, path))
            .header("Accept", BROWSER)
            .header("Authorization", format!("Token {}", f.api_token))
            .send()
            .await
            .unwrap();
        let status = resp.status();
        let body = resp.text().await.unwrap();
        assert!(status.is_client_error(), "{path}: {status}");
        assert!(
            body.contains("error_msg"),
            "{path} must keep the wire body, got: {body}"
        );
    }
}

/// A missing static asset is a broken `<img>` or `<link>`, not a page: an HTML
/// body there would be worse than the 13-byte string.
#[tokio::test]
async fn a_missing_static_asset_stays_plain_text() {
    let f = TestFixture::new().await;
    let (status, body) = browse(&f, "/static/css/no-such-file.css").await;

    assert_eq!(status, 404);
    assert_eq!(body, "404 Not Found");
}
