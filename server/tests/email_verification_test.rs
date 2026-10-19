//! Open registration and email verification.
//!
//! These tests pin the switch from invitation-only to open sign-up: the account
//! is created unverified, no session is issued, the mail is queued, and only the
//! one-time link makes the account usable.

mod common;

use common::TestServer;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

fn token_from_url(url: &str) -> String {
    url.trim_end_matches('/')
        .rsplit('/')
        .next()
        .expect("the verification URL carries a token")
        .to_string()
}

async fn register_open(server: &TestServer, email: &str, password: &str) -> reqwest::Response {
    server
        .client()
        .post_ui_form(
            "/accounts/register/",
            &[
                ("email", email),
                ("password1", password),
                ("password2", password),
            ],
        )
        .await
}

#[tokio::test]
async fn open_registration_creates_an_unverified_account_and_queues_mail() {
    let server = TestServer::start_with_open_registration().await;
    let client = server.client();

    let login = client
        .get("/accounts/login/", None)
        .await
        .text()
        .await
        .unwrap();
    assert!(
        login.contains("/accounts/register/"),
        "the create-account link must be visible while registration is available"
    );

    let page = client
        .get("/accounts/register/", None)
        .await
        .text()
        .await
        .unwrap();
    assert!(
        !page.contains("name=\"invitation_code\""),
        "open registration must not ask for an invitation code"
    );
    assert!(
        page.contains("verification link"),
        "the form must explain that a verification mail is coming"
    );

    let email = "open-new@example.com";
    let resp = register_open(&server, email, "password123").await;
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("Check your email") && body.contains(email),
        "the sent page must name the address: {body}"
    );

    let user = server
        .repos
        .user
        .find_by_email(email)
        .await
        .unwrap()
        .expect("the account exists");
    assert!(!user.email_verified, "open registration starts unverified");
    assert!(user.invited_by.is_none(), "nobody invited an open signup");
    assert!(
        user.last_login_at.is_none(),
        "registration must not count as a sign-in"
    );
    assert!(
        server
            .repos
            .api_token
            .list_sessions(user.id)
            .await
            .unwrap()
            .is_empty(),
        "no session may be minted before verification"
    );

    let tokens = server
        .repos
        .email_verification_token
        .find_by_user(user.id)
        .await
        .unwrap();
    assert_eq!(tokens.len(), 1, "one verification link was issued");
    assert!(!tokens[0].used);

    let mails = infra::entity::email_message::Entity::find()
        .filter(infra::entity::email_message::Column::Kind.eq("email_verification"))
        .all(server.db.as_ref())
        .await
        .unwrap();
    assert!(!mails.is_empty(), "the verification mail was queued");
}

#[tokio::test]
async fn an_unverified_login_gets_the_sent_page_and_no_session() {
    let server = TestServer::start_with_open_registration().await;
    let client = server.client();
    let email = "open-login@example.com";
    let password = "password123";

    let resp = register_open(&server, email, password).await;
    assert_eq!(resp.status(), 200);

    let user = server
        .repos
        .user
        .find_by_email(email)
        .await
        .unwrap()
        .unwrap();
    let before = server
        .repos
        .email_verification_token
        .find_by_user(user.id)
        .await
        .unwrap();

    let resp = client
        .post_ui_form(
            "/accounts/login/",
            &[("email", email), ("password", password)],
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("Check your email"),
        "an unverified login shows the verification page: {body}"
    );

    let user = server
        .repos
        .user
        .find_by_email(email)
        .await
        .unwrap()
        .unwrap();
    assert!(
        user.last_login_at.is_none(),
        "an unverified login must not count as a sign-in"
    );
    assert!(
        server
            .repos
            .api_token
            .list_sessions(user.id)
            .await
            .unwrap()
            .is_empty(),
        "an unverified login must not mint a session"
    );
    let after = server
        .repos
        .email_verification_token
        .find_by_user(user.id)
        .await
        .unwrap();
    assert_eq!(
        after.len(),
        1,
        "the resend cooldown must keep the outstanding link valid"
    );
    assert_eq!(after[0].created_at, before[0].created_at);

    // The API login path refuses the same account without minting a token.
    let resp = client.login(email, password).await;
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body["non_field_errors"][0]
            .as_str()
            .unwrap_or_default()
            .contains("not verified"),
        "the API must explain why it refused: {body}"
    );
}

#[tokio::test]
async fn the_verification_link_activates_the_account_and_allows_sign_in() {
    let server = TestServer::start_with_open_registration().await;
    let email = "open-verify@example.com";
    let password = "password123";

    let resp = register_open(&server, email, password).await;
    assert_eq!(resp.status(), 200);
    let user = server
        .repos
        .user
        .find_by_email(email)
        .await
        .unwrap()
        .unwrap();

    // The mail itself is encrypted in the outbox, so mint a link through the same
    // service the registration path used. This invalidates the first link.
    let service = server::service::auth::email_verification::EmailVerificationService::new(
        server.repos.clone(),
    );
    let url = service
        .issue(user.id, &server.base_url)
        .await
        .expect("issue a verification link");
    let token = token_from_url(&url);

    let page = server
        .client()
        .get(&format!("/accounts/verify-email/{token}/"), None)
        .await;
    assert_eq!(page.status(), 200);
    let body = page.text().await.unwrap();
    assert!(
        body.contains("Email verified"),
        "the success page must say so: {body}"
    );

    let user = server
        .repos
        .user
        .find_by_email(email)
        .await
        .unwrap()
        .unwrap();
    assert!(user.email_verified, "the link marks the address verified");

    // A URL is one-shot.
    let again = server
        .client()
        .get(&format!("/accounts/verify-email/{token}/"), None)
        .await;
    assert_eq!(again.status(), 200);
    let body = again.text().await.unwrap();
    assert!(
        body.contains("Verification link invalid"),
        "a reused link must be refused: {body}"
    );

    let unknown = "a".repeat(64);
    let body = server
        .client()
        .get(&format!("/accounts/verify-email/{unknown}/"), None)
        .await
        .text()
        .await
        .unwrap();
    assert!(body.contains("Verification link invalid"));

    // The account can now sign in through the Web UI.
    let no_redirect = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let resp = no_redirect
        .post(format!("{}/accounts/login/", server.base_url))
        .form(&[("email", email), ("password", password)])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 302, "a verified account signs in");
    assert_eq!(
        resp.headers().get("location").and_then(|v| v.to_str().ok()),
        Some("/libraries/")
    );
    let user = server
        .repos
        .user
        .find_by_email(email)
        .await
        .unwrap()
        .unwrap();
    assert!(user.last_login_at.is_some());
}

#[tokio::test]
async fn open_registration_without_mail_is_closed_even_for_the_master_switch() {
    let server = TestServer::start_with_auth_config(|auth| auth.require_invitation = false).await;
    let client = server.client();

    let login = client
        .get("/accounts/login/", None)
        .await
        .text()
        .await
        .unwrap();
    assert!(
        !login.contains("/accounts/register/"),
        "no create-account link can work without verification mail"
    );

    let page = client.get("/accounts/register/", None).await;
    assert_eq!(page.status(), 404, "the page must fail closed");

    let resp = register_open(&server, "no-mail@example.com", "password123").await;
    assert_eq!(resp.status(), 404, "the POST must fail closed too");
    assert!(
        server
            .repos
            .user
            .find_by_email("no-mail@example.com")
            .await
            .unwrap()
            .is_none(),
        "a refused registration must not create an account"
    );
}

#[tokio::test]
async fn the_master_switch_closes_open_registration() {
    let server = TestServer::start_with_auth_config(|auth| {
        auth.require_invitation = false;
        auth.enable_registration = false;
    })
    .await;
    let client = server.client();

    assert_eq!(client.get("/accounts/register/", None).await.status(), 404);
    assert_eq!(
        register_open(&server, "closed@example.com", "password123")
            .await
            .status(),
        404
    );
}

#[tokio::test]
async fn an_unverified_account_cannot_use_a_token_issued_out_of_band() {
    let server = TestServer::start_with_open_registration().await;
    let email = "open-token@example.com";
    register_open(&server, email, "password123").await;
    let user = server
        .repos
        .user
        .find_by_email(email)
        .await
        .unwrap()
        .unwrap();

    let raw = server::service::auth::token::generate_api_token();
    server
        .repos
        .api_token
        .create_session_token(server::repository::api_token::CreateSessionTokenParams {
            user_id: user.id,
            token: raw.clone(),
            created_at: chrono::Utc::now().timestamp(),
            expires_at: None,
            device_id: None,
            platform: None,
            device_name: None,
            client_version: None,
            is_pending: false,
            source: server::domain::session_source::SessionSource::Web,
            user_agent: None,
        })
        .await
        .unwrap();

    let resp = server.client().get("/api2/repos/", Some(&raw)).await;
    assert_eq!(
        resp.status(),
        403,
        "the credential layer must also require verification"
    );
}

#[tokio::test]
async fn stale_unverified_accounts_are_removed_but_verified_ones_stay() {
    let server = TestServer::start().await;
    let stale = common::create_test_user(&server.db, "stale@example.com", "password").await;
    let verified = common::create_test_user(&server.db, "verified@example.com", "password").await;
    let old = chrono::Utc::now().timestamp() - 8 * 24 * 60 * 60;

    use sea_orm::{ActiveModelTrait, Set};
    for (id, email_verified) in [(stale, false), (verified, true)] {
        let mut active: infra::entity::user::ActiveModel =
            infra::entity::user::Entity::find_by_id(id)
                .one(server.db.as_ref())
                .await
                .unwrap()
                .unwrap()
                .into();
        active.email_verified = Set(email_verified);
        active.created_at = Set(old);
        active.update(server.db.as_ref()).await.unwrap();
    }

    let removed = server
        .repos
        .user
        .delete_unverified_older_than(old + 1)
        .await
        .unwrap();
    assert_eq!(removed, 1, "only the old unverified account goes");
    assert!(
        server
            .repos
            .user
            .find_by_email("stale@example.com")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        server
            .repos
            .user
            .find_by_email("verified@example.com")
            .await
            .unwrap()
            .is_some()
    );
}
