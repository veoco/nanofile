//! Every account token records which login produced it.
//!
//! `api_tokens.source` exists because `platform` could not answer that question:
//! it is set only when a client reports device details, so a browser session, a
//! client that sent nothing, and the desktop client's "view on website" handoff
//! all looked alike. The credential inventory groups by `source`, so a wrong
//! label is a credential filed in the wrong place — and a credential filed
//! nowhere is invisible.

mod common;

use common::TestFixture;
use infra::entity::api_token;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use server::service::auth::token::hash_token;

/// The row behind a raw token.
async fn row_for(f: &TestFixture, raw_token: &str) -> api_token::Model {
    api_token::Entity::find()
        .filter(api_token::Column::Token.eq(hash_token(raw_token)))
        .one(f.server.db.as_ref())
        .await
        .expect("query api_tokens")
        .expect("the token must exist")
}

/// Every token row for the fixture's user with the given source.
async fn rows_with_source(f: &TestFixture, source: &str) -> Vec<api_token::Model> {
    api_token::Entity::find()
        .filter(api_token::Column::UserId.eq(f.user_id))
        .filter(api_token::Column::Source.eq(source))
        .all(f.server.db.as_ref())
        .await
        .expect("query api_tokens")
}

/// A cookie-holding browser, labelled with a known `User-Agent`.
fn browser(user_agent: &str) -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .cookie_store(true)
        .user_agent(user_agent)
        .build()
        .expect("client")
}

/// Regression: the fixture logs in through `/api2/auth-token/` with only a
/// username and password, so the row carries no `platform`. Before `source`
/// existed that made it indistinguishable from a browser session, and the
/// inventory left it out entirely.
#[tokio::test]
async fn a_client_that_reports_no_device_details_is_still_a_client() {
    let f = TestFixture::new().await;
    let row = row_for(&f, &f.api_token).await;

    assert_eq!(row.source, "client");
    assert!(
        row.platform.is_none(),
        "the fixture deliberately sends no device details"
    );
}

#[tokio::test]
async fn a_client_that_reports_device_details_records_them() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .login_multipart("test@example.com", "password")
        .await;
    let token = resp.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .expect("token")
        .to_string();

    let row = row_for(&f, &token).await;
    assert_eq!(row.source, "client");
    assert_eq!(row.platform.as_deref(), Some("android"));
    assert_eq!(row.device_id.as_deref(), Some("test-device-123"));
}

/// A browser session has no device name, so the `User-Agent` is the only thing
/// that tells the owner which session a row is.
#[tokio::test]
async fn a_web_login_records_its_user_agent() {
    let f = TestFixture::new().await;
    let agent = "NanofileTestBrowser/1.0 (X11; Linux x86_64)";
    let client = browser(agent);

    let resp = client
        .post(format!("{}/accounts/login/", f.server.base_url))
        .form(&[("email", "test@example.com"), ("password", "password")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 302, "the web login must succeed");

    let sessions = rows_with_source(&f, "web").await;
    assert_eq!(sessions.len(), 1, "one browser session was created");
    assert_eq!(sessions[0].user_agent.as_deref(), Some(agent));
    assert!(
        sessions[0].platform.is_none(),
        "a browser session has no device"
    );
    assert!(!sessions[0].is_pending);
}

/// The desktop client's "view on website" opens a real browser session — it
/// sets the session cookie — but the user never typed a password to get it.
/// That is exactly the distinction the inventory has to make.
#[tokio::test]
async fn the_view_on_website_handoff_is_a_browser_session() {
    let f = TestFixture::new().await;

    let issued = f
        .client
        .post_json(
            "/api2/client-login/",
            Some(&f.api_token),
            &serde_json::json!({}),
        )
        .await;
    assert_eq!(issued.status(), 200, "the client mints a one-time token");
    let one_time = issued.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .expect("token")
        .to_string();

    let agent = "NanofileTestBrowser/1.0 (handoff)";
    let client = browser(agent);
    let opened = client
        .get(format!(
            "{}/client-login/?token={one_time}",
            f.server.base_url
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        opened.status(),
        302,
        "a trusted same-address handoff signs the browser in"
    );

    let sessions = rows_with_source(&f, "web_client_login").await;
    assert_eq!(sessions.len(), 1, "the handoff session was created");
    assert_eq!(sessions[0].user_agent.as_deref(), Some(agent));

    // It is a browser session, not a client device: nothing about it should
    // reach the list of connected devices.
    assert!(sessions[0].platform.is_none());
    assert!(sessions[0].device_id.is_none());
}
