//! SSO / client-login hardening.
//!
//! The SSO flow is anonymous, hands a client full account access, and used to
//! keep a link redeemable indefinitely (its `expires_at` was written and never
//! read) with no cap on link creation.

mod common;

use common::{TestServer, create_test_user};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};

/// Anonymous link creation is capped per client address: each call writes a row,
/// so an uncapped endpoint is a cheap way to fill `sso_login_tokens`.
#[tokio::test]
async fn sso_link_creation_is_rate_limited() {
    let server = TestServer::start_with_auth_config(|auth| auth.sso_link_max_per_hour = 2).await;
    let client = server.client();

    for attempt in 0..2 {
        let resp = client.post_form("/api2/client-sso-link/", None, &[]).await;
        assert_eq!(resp.status(), 200, "attempt {attempt} should be allowed");
    }

    let resp = client.post_form("/api2/client-sso-link/", None, &[]).await;
    assert_eq!(
        resp.status(),
        429,
        "the third creation must be throttled, got {}",
        resp.status()
    );
}

/// A parameterless POST is the mobile clients' shape and must keep working: only
/// the desktop client sends `shib_*` device parameters.
#[tokio::test]
async fn sso_link_accepts_a_parameterless_post() {
    let server = TestServer::start_with_auth_config(|auth| auth.sso_link_max_per_hour = 50).await;
    let client = server.client();

    for (query, label) in [
        ("", "mobile (no params)"),
        (
            "?shib_platform=linux&shib_device_name=laptop&shib_client_version=9.0.21",
            "desktop",
        ),
    ] {
        let resp = client
            .post_form(&format!("/api2/client-sso-link/{query}"), None, &[])
            .await;
        assert_eq!(resp.status(), 200, "{label} POST must work");
        let body: serde_json::Value = resp.json().await.unwrap();
        let link = body["link"].as_str().unwrap_or_default();
        assert!(
            link.contains("/client-sso/"),
            "{label}: the link must contain the token path, got {link}"
        );
    }
}

/// An expired link is dead. Polling answers 404, which is the only shape that
/// stops the desktop, Android and iOS clients alike (they keep re-polling a
/// non-"success" status body).
#[tokio::test]
async fn expired_sso_link_stops_resolving() {
    let server = TestServer::start_with_auth_config(|auth| auth.sso_link_max_per_hour = 50).await;
    let client = server.client();

    let resp = client.post_form("/api2/client-sso-link/", None, &[]).await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let link = body["link"].as_str().unwrap().to_string();
    let token = link
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap()
        .to_string();

    // Before expiry the poll reports "waiting".
    let resp = client
        .get(&format!("/api2/client-sso-link/{token}/"), None)
        .await;
    assert_eq!(resp.status(), 200, "a fresh link must poll");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "waiting");

    // Age the link past its `expires_at` (one hour) directly.
    let model = infra::entity::sso_login_token::Entity::find()
        .filter(
            infra::entity::sso_login_token::Column::Token
                .eq(server::service::auth::token::hash_token(&token)),
        )
        .one(&*server.db)
        .await
        .unwrap()
        .expect("link row");
    let mut active: infra::entity::sso_login_token::ActiveModel = model.into();
    active.expires_at = Set(Some(chrono::Utc::now().timestamp() - 1));
    active.update(&*server.db).await.unwrap();

    // The poll now 404s (and the row is gone).
    let resp = client
        .get(&format!("/api2/client-sso-link/{token}/"), None)
        .await;
    assert_eq!(
        resp.status(),
        404,
        "an expired link must answer 404 so every client stops polling, got {}",
        resp.status()
    );

    // The browser entry point refuses it too.
    let resp = client.get(&format!("/client-sso/{token}/"), None).await;
    assert_eq!(
        resp.status(),
        400,
        "the browser page must show the error page, got {}",
        resp.status()
    );
}

/// The confirmation page names the client that asked for access, so a user can
/// notice a link they did not start. Without it the page is a generic
/// "authorize this device?" prompt, which is what makes the flow phishable.
#[tokio::test]
async fn sso_confirm_page_shows_the_requester() {
    let server = TestServer::start_with_auth_config(|auth| auth.sso_link_max_per_hour = 50).await;
    create_test_user(&server.db, "user@example.com", "password123").await;
    let client = server.client();

    // The desktop client's shape: device parameters on the query string.
    let resp = client
        .post_form(
            "/api2/client-sso-link/?shib_platform=linux&shib_device_name=workstation&shib_client_version=9.0.21",
            None,
            &[],
        )
        .await;
    let link: String = resp.json::<serde_json::Value>().await.unwrap()["link"]
        .as_str()
        .unwrap()
        .to_string();
    let token = link
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap()
        .to_string();

    // A browser session: log in with cookies and no redirect following.
    let browser = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .cookie_store(true)
        .build()
        .unwrap();
    let base = server.base_url.clone();
    let resp = browser
        .post(format!("{base}/accounts/login/"))
        .form(&[("email", "user@example.com"), ("password", "password123")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 302, "browser login failed");

    // Opening the link marks it accessed (it also redirects to the login page,
    // which is already satisfied).
    let _ = browser
        .get(format!("{base}/client-sso/{token}/"))
        .send()
        .await;

    let body = browser
        .get(format!("{base}/client-sso/{token}/complete/"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        body.contains("workstation"),
        "the confirmation page must name the requesting device; got: {body}"
    );
    assert!(
        body.contains("linux"),
        "the confirmation page must name the requesting platform"
    );
}
