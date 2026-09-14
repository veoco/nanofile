//! The bulk credential actions.
//!
//! "Sign out my other sessions" is the one action on the credentials page that
//! can lock the actor out of their own account, so what these tests pin is the
//! opposite: the acting session survives, and nothing outside the intended set
//! is touched. They drive the plain HTML form, since the page has to work
//! without JavaScript.

mod common;

use common::TestFixture;
use infra::entity::api_token;
use sea_orm::{ActiveModelTrait, Set};

/// A browser session row with an explicit source, inserted directly so the
/// test controls the set the bulk action sees.
async fn seed_browser_session(f: &TestFixture, user_id: i32, source: &str) -> i32 {
    let raw = server::service::auth::token::generate_api_token();
    let model = api_token::ActiveModel {
        id: sea_orm::NotSet,
        user_id: Set(user_id),
        token: Set(server::service::auth::token::hash_token(&raw)),
        created_at: Set(1_000),
        expires_at: Set(None),
        device_id: Set(None),
        platform: Set(None),
        device_name: Set(None),
        client_version: Set(None),
        is_pending: Set(false),
        source: Set(source.to_string()),
        user_agent: Set(Some("NanofileTestBrowser/1.0".to_string())),
    };
    model
        .insert(f.server.db.as_ref())
        .await
        .expect("insert browser session")
        .id
}

/// A cookie-holding client (the Web UI authenticates with a session cookie).
async fn login_client(fixture: &TestFixture) -> reqwest::Client {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .cookie_store(true)
        .user_agent("NanofileTestBrowser/1.0")
        .build()
        .unwrap();
    let resp = client
        .post(format!("{}/accounts/login/", fixture.server.base_url))
        .form(&[("email", "test@example.com"), ("password", "password")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 302, "login for test helper");
    client
}

fn csrf_token(body: &str) -> String {
    let marker = r#"name="csrf_token" value=""#;
    body.find(marker)
        .and_then(|i| {
            let rest = &body[i + marker.len()..];
            let end = rest.find('"')?;
            Some(rest[..end].to_string())
        })
        .unwrap_or_default()
}

async fn page(client: &reqwest::Client, base_url: &str) -> String {
    client
        .get(format!("{base_url}/settings/credentials/"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap()
}

/// The bulk form posts, the acting session survives, and the page says how many
/// rows it removed.
#[tokio::test]
async fn signing_out_other_sessions_keeps_the_acting_one() {
    let f = TestFixture::new().await;
    // Two other browser sessions and one client session that must not be
    // signed out by a browser action.
    seed_browser_session(&f, f.user_id, "web").await;
    seed_browser_session(&f, f.user_id, "web_client_login").await;

    let client = login_client(&f).await;
    let body = page(&client, &f.server.base_url).await;
    let token = csrf_token(&body);
    assert!(!token.is_empty(), "the page embeds a csrf token");

    // The page offers the action only because there are others to sign out.
    assert!(
        body.contains(r#"name="action" value="sign_out_others""#),
        "the bulk form is offered"
    );

    let resp = client
        .post(format!("{}/settings/credentials/bulk/", f.server.base_url))
        .form(&[
            ("csrf_token", token.as_str()),
            ("action", "sign_out_others"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 302);
    assert_eq!(
        resp.headers().get("location").unwrap(),
        "/settings/credentials/?bulk=sign_out_others&n=2",
        "the redirect reports what was removed"
    );

    // The browser session this test is holding is still usable: the page loads
    // and still marks it as the current session.
    let after = page(&client, &f.server.base_url).await;
    assert!(
        after.contains("This session"),
        "the acting session survives"
    );

    let sources: Vec<String> = f
        .server
        .repos
        .api_token
        .list_sessions(f.user_id)
        .await
        .expect("list")
        .into_iter()
        .map(|s| s.source)
        .collect();
    assert_eq!(
        sources.iter().filter(|s| *s == "web").count(),
        1,
        "only the acting browser session is left: {sources:?}"
    );
    assert!(
        !sources.iter().any(|s| s == "web_client_login"),
        "the desktop client's handoff session is signed out too: {sources:?}"
    );
}

/// Revoking every sync token empties the list and reports the count.
#[tokio::test]
async fn revoking_all_sync_tokens_empties_the_list() {
    let f = TestFixture::new().await;
    // The fixture already holds one sync token (minted through the API).
    assert_eq!(
        f.server
            .repos
            .sync_token
            .list_for_user(f.user_id)
            .await
            .expect("list")
            .len(),
        1
    );

    let client = login_client(&f).await;
    let body = page(&client, &f.server.base_url).await;
    let resp = client
        .post(format!("{}/settings/credentials/bulk/", f.server.base_url))
        .form(&[
            ("csrf_token", csrf_token(&body).as_str()),
            ("action", "revoke_sync_tokens"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 302);
    assert_eq!(
        resp.headers().get("location").unwrap(),
        "/settings/credentials/?bulk=revoke_sync_tokens&n=1"
    );

    assert!(
        f.server
            .repos
            .sync_token
            .list_for_user(f.user_id)
            .await
            .expect("list")
            .is_empty(),
        "every sync token is gone"
    );
    assert!(
        page(&client, &f.server.base_url)
            .await
            .contains("No sync tokens."),
        "the page says there are none left"
    );
}

/// An action that found nothing to do says so rather than reporting a count of
/// zero as if something had happened.
#[tokio::test]
async fn a_no_op_bulk_action_reports_nothing_to_revoke() {
    let f = TestFixture::new().await;
    let client = login_client(&f).await;
    let body = page(&client, &f.server.base_url).await;

    let resp = client
        .post(format!("{}/settings/credentials/bulk/", f.server.base_url))
        .form(&[
            ("csrf_token", csrf_token(&body).as_str()),
            ("action", "sign_out_others"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 302);
    assert_eq!(
        resp.headers().get("location").unwrap(),
        "/settings/credentials/?bulk=nothing"
    );
    assert!(
        page(&client, &f.server.base_url)
            .await
            .contains("nothing left to revoke"),
        "the page explains the no-op"
    );
}

/// A missing CSRF token, and an action this build does not know, are both
/// refused before anything is deleted.
#[tokio::test]
async fn bulk_actions_require_csrf_and_a_known_action() {
    let f = TestFixture::new().await;
    seed_browser_session(&f, f.user_id, "web").await;
    let client = login_client(&f).await;
    let body = page(&client, &f.server.base_url).await;
    let token = csrf_token(&body);

    let no_csrf = client
        .post(format!("{}/settings/credentials/bulk/", f.server.base_url))
        .form(&[("action", "sign_out_others")])
        .send()
        .await
        .unwrap();
    assert!(
        no_csrf.status().is_client_error(),
        "a missing csrf token is refused, got {}",
        no_csrf.status()
    );

    let unknown = client
        .post(format!("{}/settings/credentials/bulk/", f.server.base_url))
        .form(&[
            ("csrf_token", token.as_str()),
            ("action", "delete_everything"),
        ])
        .send()
        .await
        .unwrap();
    assert!(
        unknown.status().is_client_error(),
        "an unknown action is refused, got {}",
        unknown.status()
    );

    assert_eq!(
        f.server
            .repos
            .api_token
            .list_sessions(f.user_id)
            .await
            .expect("list")
            .iter()
            .filter(|s| s.source == "web")
            .count(),
        2,
        "nothing was deleted by either refused request"
    );
}

/// The credentials page must not offer "sign out other sessions" when the
/// acting session is the only one — an action that would do nothing invites a
/// click that appears broken.
#[tokio::test]
async fn the_bulk_action_is_hidden_when_there_is_nothing_to_do() {
    let f = TestFixture::new().await;
    let client = login_client(&f).await;
    let body = page(&client, &f.server.base_url).await;
    assert!(
        !body.contains(r#"name="action" value="sign_out_others""#),
        "no other browser session exists, so the action is not offered"
    );
}
