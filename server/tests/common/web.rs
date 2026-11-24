//! Web-page helpers for the integration tests that drive the real router.
//!
//! Several page tests each built a cookie-store client, logged in through
//! `/accounts/login/`, fetched a page and split off the `#__i18n` dictionary.
//! The prompt/redirect/CSRF conventions live here instead.

use reqwest::Client;

use crate::common::TestServer;
use crate::common::client::csrf_token;

/// Log in through the Web UI form and keep the session cookies.
///
/// Redirects are not followed, so a page that turns a request away reports its
/// own status. The `/libraries/` warm-up mirrors what a browser does after login.
pub async fn ui_login(server: &TestServer, email: &str, password: &str) -> Client {
    let client = Client::builder()
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .unwrap();
    let resp = client
        .post(format!("{}/accounts/login/", server.base_url))
        .form(&[("email", email), ("password", password)])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 302, "login should redirect");
    let _ = client
        .get(format!("{}/libraries/", server.base_url))
        .send()
        .await;
    client
}

/// Fetch a page and return its status plus body.
pub async fn page(server: &TestServer, client: &Client, path: &str) -> (u16, String) {
    let resp = client
        .get(format!(
            "{}/{}",
            server.base_url,
            path.trim_start_matches('/')
        ))
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    (status, resp.text().await.unwrap())
}

/// Everything the page renders, without the translation dictionary and the
/// bundle script that follow it.
///
/// The dictionary ships every key with its translation, so a raw key there is
/// normal; a raw key in the visible body is not. A page without the marker is a
/// test bug, so this panics rather than returning the whole page (the previous
/// `unwrap_or(html)` fallback could turn a missing-body assertion into a pass).
pub fn page_content(html: &str) -> &str {
    html.split("id=\"__i18n\"")
        .next()
        .filter(|body| !body.is_empty())
        .unwrap_or_else(|| panic!("page has no id=\"__i18n\" dictionary block"))
}

/// The page's CSRF token; panics when the form does not carry one.
pub fn csrf_of(html: &str) -> String {
    csrf_token(html).expect("page must embed a csrf_token")
}
