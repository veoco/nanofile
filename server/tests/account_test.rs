use crate::common;

use common::{TestFixture, TestServer, create_test_user};
use serde_json::Value;

#[tokio::test]
async fn test_account_info_returns_display_name() {
    let f = TestFixture::no_repo("test@example.com", "password").await;

    use sea_orm::{ActiveModelTrait, EntityTrait, Set};
    let user_record = infra::entity::user::Entity::find_by_id(f.user_id)
        .one(&*f.server.db)
        .await
        .unwrap()
        .unwrap();
    let mut active: infra::entity::user::ActiveModel = user_record.into();
    active.display_name = Set(Some("Alice".to_string()));
    active.update(&*f.server.db).await.unwrap();

    let resp = f
        .client
        .get("/api2/account/info/", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "Alice");
    assert_eq!(body["nickname"], "Alice");
    assert_eq!(body["email"], "test@example.com");
    assert!(body["usage"].is_number());
    assert!(body["total"].is_number());
}

/// neither display_name nor name is set.
#[tokio::test]
async fn test_account_info_falls_back_to_email_local_part() {
    let server = TestServer::start().await;
    let client = server.client();
    let _user_id = create_test_user(&server.db, "john.doe@example.com", "password").await;

    let resp = client.login("john.doe@example.com", "password").await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    let resp = client.get("/api2/account/info/", Some(token)).await;
    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "john.doe");
    assert_eq!(body["nickname"], "john.doe");
}

#[tokio::test]
async fn test_account_info_uses_name_when_display_name_unset() {
    let server = TestServer::start().await;
    let client = server.client();

    use infra::entity::user;
    use sea_orm::{ActiveModelTrait, EntityTrait, Set};

    let user_id = create_test_user(&server.db, "bob@example.com", "password").await;
    let user_record = user::Entity::find_by_id(user_id)
        .one(&*server.db)
        .await
        .unwrap()
        .unwrap();
    let mut active: user::ActiveModel = user_record.into();
    active.name = Set(Some("Bob".to_string())); // set name, no display_name
    active.display_name = Set(None);
    active.update(&*server.db).await.unwrap();

    let resp = client.login("bob@example.com", "password").await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    let resp = client.get("/api2/account/info/", Some(token)).await;
    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "Bob");
    assert_eq!(body["nickname"], "Bob");
}

#[tokio::test]
async fn test_put_account_info_updates_display_name() {
    let f = TestFixture::no_repo("test@example.com", "password").await;

    let resp = f
        .client
        .put_json(
            "/api2/account/info/",
            Some(&f.api_token),
            &serde_json::json!({"name": "My Display Name"}),
        )
        .await;
    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "My Display Name");

    let resp = f
        .client
        .get("/api2/account/info/", Some(&f.api_token))
        .await;
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "My Display Name");
}

/// falling back to the email local part.
#[tokio::test]
async fn test_put_account_info_empty_name_clears_display_name() {
    let f = TestFixture::no_repo("test@example.com", "password").await;

    f.client
        .put_json(
            "/api2/account/info/",
            Some(&f.api_token),
            &serde_json::json!({"name": "Temporary"}),
        )
        .await;

    let resp = f
        .client
        .put_json(
            "/api2/account/info/",
            Some(&f.api_token),
            &serde_json::json!({"name": ""}),
        )
        .await;
    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "test");
}

#[tokio::test]
async fn test_account_info_unauthorized() {
    let server = TestServer::start().await;
    let client = server.client();

    let resp = client.get("/api2/account/info/", None).await;
    assert_eq!(resp.status(), 401);

    let resp = client
        .put_json(
            "/api2/account/info/",
            None,
            &serde_json::json!({"name": "x"}),
        )
        .await;
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_account_info_quota_unlimited() {
    let f = TestFixture::no_repo("test@example.com", "password").await;

    use sea_orm::{ActiveModelTrait, EntityTrait, Set};
    let user_record = infra::entity::user::Entity::find_by_id(f.user_id)
        .one(&*f.server.db)
        .await
        .unwrap()
        .unwrap();
    let mut active: infra::entity::user::ActiveModel = user_record.into();
    active.storage_quota = Set(Some(0));
    active.update(&*f.server.db).await.unwrap();

    let resp = f
        .client
        .get("/api2/account/info/", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["total"], -1);
}

#[tokio::test]
async fn test_account_info_quota_user_specific() {
    let f = TestFixture::no_repo("test@example.com", "password").await;

    use sea_orm::{ActiveModelTrait, EntityTrait, Set};
    let user_record = infra::entity::user::Entity::find_by_id(f.user_id)
        .one(&*f.server.db)
        .await
        .unwrap()
        .unwrap();
    let mut active: infra::entity::user::ActiveModel = user_record.into();
    active.storage_quota = Set(Some(1_073_741_824)); // 1 GB
    active.update(&*f.server.db).await.unwrap();

    let resp = f
        .client
        .get("/api2/account/info/", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["total"], 1_073_741_824);
}

#[tokio::test]
async fn test_account_info_quota_fallback_default() {
    let f = TestFixture::no_repo("test@example.com", "password").await;

    // storage_quota is None by default -> the server's default space quota (10 GB).
    let resp = f
        .client
        .get("/api2/account/info/", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["total"], 10_737_418_240_i64); // 10 GB from test config
}

// Account registration requires admin authentication

/// Security: anonymous registration via POST /api2/accounts/ must be rejected
/// (it previously bypassed the invitation system and rate limits entirely).
#[tokio::test]
async fn test_register_anonymous_rejected() {
    let server = TestServer::start().await;
    let client = server.client();

    let resp = client
        .post_form(
            "/api2/accounts/",
            None,
            &[("email", "anon@example.com"), ("password", "secret123")],
        )
        .await;
    assert_eq!(resp.status(), 401);
}

/// Security: a logged-in non-admin user must not be able to create accounts.
#[tokio::test]
async fn test_register_non_admin_rejected() {
    let server = TestServer::start().await;
    let client = server.client();
    create_test_user(&server.db, "user@example.com", "password").await;

    let resp = client.login("user@example.com", "password").await;
    let body: Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    let resp = client
        .post_form(
            "/api2/accounts/",
            Some(&token),
            &[("email", "new@example.com"), ("password", "secret123")],
        )
        .await;
    assert_eq!(resp.status(), 403);
}

/// Security: an admin may create accounts via the API (seahub behaviour).
#[tokio::test]
async fn test_register_admin_allowed() {
    use sea_orm::{ActiveModelTrait, EntityTrait, Set};

    let server = TestServer::start().await;
    let client = server.client();
    let uid = create_test_user(&server.db, "admin@example.com", "password").await;

    let user_record = infra::entity::user::Entity::find_by_id(uid)
        .one(&*server.db)
        .await
        .unwrap()
        .unwrap();
    let mut active: infra::entity::user::ActiveModel = user_record.into();
    active.is_admin = Set(true);
    active.update(&*server.db).await.unwrap();

    let resp = client.login("admin@example.com", "password").await;
    let body: Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    let resp = client
        .post_form(
            "/api2/accounts/",
            Some(&token),
            &[("email", "created@example.com"), ("password", "secret123")],
        )
        .await;
    assert_eq!(resp.status(), 200);

    let resp = client.login("created@example.com", "secret123").await;
    assert_eq!(resp.status(), 200);
}

// ── Self-registration in invitation mode ────────────────────────────────────
//
// The invitation code is the trust proof in this mode, but the address is still
// the account's login name and may receive notification mail, so it must pass
// the same parser open registration uses. It is also stored trimmed: an
// untrimmed login name could never be matched by the address the user types.

/// Seed an invitation code owned by the fixture user.
async fn seed_invitation_code(f: &TestFixture, code: &str, bound_email: Option<&str>) {
    f.server
        .repos
        .invitation_code
        .create(
            code.to_string(),
            bound_email.map(str::to_string),
            f.user_id,
            chrono::Utc::now().timestamp(),
        )
        .await
        .expect("seed an invitation code");
}

/// Submit the registration form in invitation mode.
///
/// No redirect following: a successful registration answers `302` to
/// `/libraries/`, and following it would turn the success into a login page.
async fn register_with_invitation(f: &TestFixture, email: &str, code: &str) -> reqwest::Response {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .post(format!("{}/accounts/register/", f.server.base_url))
        .form(&[
            ("email", email),
            ("password1", "password123"),
            ("password2", "password123"),
            ("invitation_code", code),
        ])
        .send()
        .await
        .unwrap()
}

/// Invitation mode used to accept any string containing "@" as an address, so a
/// malformed login name could be created (and later mailed to). It now runs the
/// same parser as open registration, and a refused address does not burn the
/// invitation code.
#[tokio::test]
async fn test_invitation_registration_rejects_invalid_email() {
    let f = TestFixture::new().await;
    let code = "invite-invalid-email-0123456789abcdef";
    seed_invitation_code(&f, code, None).await;

    for invalid in ["not-an-address", "a@b@c", "user name@example.com"] {
        let resp = register_with_invitation(&f, invalid, code).await;
        assert_eq!(
            resp.status(),
            200,
            "{invalid:?} must re-render the registration form"
        );
        let body = resp.text().await.unwrap();
        assert!(
            body.contains("Invalid email address."),
            "{invalid:?} must be refused with the address error, got: {body}"
        );
    }

    // The refusals did not consume the code...
    let record = f
        .server
        .repos
        .invitation_code
        .find_by_code(code)
        .await
        .unwrap()
        .expect("the invitation row");
    assert!(
        record.used_by.is_none(),
        "a rejected address must not consume the invitation code"
    );

    // ...so a valid address still registers with the same code.
    let resp = register_with_invitation(&f, "invited@example.com", code).await;
    assert_eq!(
        resp.status(),
        302,
        "a valid invitation registration signs the new account in"
    );
}

#[tokio::test]
async fn test_invitation_registration_trims_email_and_matches_the_bound_code() {
    let f = TestFixture::new().await;
    // A legacy row may carry whitespace and mixed case; the binding comparison
    // trims and case-folds both sides, like a freshly generated code (which is
    // trimmed when stored).
    let code = "invite-bound-email-0123456789abcdef";
    seed_invitation_code(&f, code, Some(" Bound@Example.com ")).await;

    let resp = register_with_invitation(&f, "  BOUND@Example.com  ", code).await;
    assert_eq!(
        resp.status(),
        302,
        "the trimmed, case-folded address matches the bound invitation"
    );

    let stored = f
        .server
        .repos
        .user
        .find_by_email("BOUND@Example.com")
        .await
        .unwrap()
        .expect("the account is stored under the trimmed address");
    assert_eq!(stored.email, "BOUND@Example.com");
    assert!(
        f.server
            .repos
            .user
            .find_by_email("  BOUND@Example.com  ")
            .await
            .unwrap()
            .is_none(),
        "the whitespace-padded spelling must not be stored as-is"
    );
}
