//! Server-wide kill switches must fail closed.
//!
//! Each case here pinned a switch that was documented as closing a feature but
//! was never consulted on the server side: `auth.enable_invitations` had no
//! reader at all, `auth.enable_password_reset` only reached the login template,
//! and `server.share_link_enabled` was skipped by the upload-token mint and by
//! the token consumers.

mod common;

use common::TestServer;

/// `auth.enable_invitations = false` closes registration: the page, the POST and
/// the login-page link all go away. A valid invitation code must not help.
#[tokio::test]
async fn disabled_invitations_close_registration() {
    let server = TestServer::start_with_auth_config(|auth| auth.enable_invitations = false).await;
    let client = server.client();

    // The login page no longer advertises registration.
    let body = client
        .get("/accounts/login/", None)
        .await
        .text()
        .await
        .unwrap();
    assert!(
        !body.contains("/accounts/register/"),
        "the create-account link must be hidden when invitations are disabled"
    );

    // The registration page itself is gone.
    let resp = client.get("/accounts/register/", None).await;
    assert_eq!(
        resp.status(),
        404,
        "the registration page must be closed, got {}",
        resp.status()
    );

    // And the POST cannot be used to register, even with a valid code: mint one
    // directly, then try.
    let admin_id = common::create_test_admin(&server.db, "admin@example.com", "password123").await;
    assert!(admin_id > 0);
    let invitation_code = "0123456789abcdef0123456789abcdef".to_string();
    server
        .repos
        .invitation_code
        .create(
            invitation_code.clone(),
            None,
            admin_id,
            chrono::Utc::now().timestamp(),
        )
        .await
        .unwrap();

    let resp = client
        .post_form(
            "/accounts/register/",
            None,
            &[
                ("email", "new@example.com"),
                ("password1", "password123"),
                ("password2", "password123"),
                ("invitation_code", invitation_code.as_str()),
            ],
        )
        .await;
    assert_eq!(
        resp.status(),
        404,
        "registration must be refused even with a valid invitation code, got {}",
        resp.status()
    );
}

/// `auth.enable_password_reset = false` closes both the request and the confirm
/// endpoints, and no reset token is minted.
#[tokio::test]
async fn disabled_password_reset_mints_nothing() {
    let server =
        TestServer::start_with_auth_config(|auth| auth.enable_password_reset = false).await;
    let client = server.client();

    let resp = client
        .post_form(
            "/accounts/password/reset/",
            None,
            &[("email", "someone@example.com")],
        )
        .await;
    // The generic "we sent you a link" page is fine; what matters is that no
    // token exists afterwards.
    assert!(
        resp.status().is_success() || resp.status().is_redirection(),
        "the reset request must not error out, got {}",
        resp.status()
    );
    let tokens = server
        .repos
        .password_reset_token
        .find_by_user(1)
        .await
        .unwrap();
    assert!(
        tokens.is_empty(),
        "no reset token may be minted while the feature is disabled"
    );

    // The confirm page is closed too.
    let resp = client
        .get("/accounts/password/reset/sometoken/", None)
        .await;
    assert_eq!(
        resp.status(),
        404,
        "the reset confirm page must be closed, got {}",
        resp.status()
    );
}

/// With anonymous links disabled, an upload URL that was already handed out must
/// stop accepting writes — the switch used to guard only the pages and the link
/// creation, so a token in its one-hour TTL kept working.
#[tokio::test]
async fn disabled_share_links_stop_minting_and_using_upload_tokens() {
    let server =
        TestServer::start_with_server_info_config(|cfg| cfg.share_link_enabled = false).await;
    let client = server.client();

    // Owner + repo on this server.
    let user_id = common::create_test_user(&server.db, "owner@example.com", "password123").await;
    assert!(user_id > 0);
    let login = client.login("owner@example.com", "password123").await;
    assert_eq!(login.status(), 200);
    let token = login.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();
    let repo_id = common::create_test_repo(&client, &token, "lib").await;

    // The anonymous entry points are refused.
    let resp = client
        .post_json(
            "/api/v2.1/upload-links/",
            Some(&token),
            &serde_json::json!({"repo_id": repo_id, "path": "/"}),
        )
        .await;
    assert_eq!(
        resp.status(),
        403,
        "creating an upload link must be refused, got {}",
        resp.status()
    );

    let resp = client
        .get("/api/v2.1/upload-links/whatever/upload/", None)
        .await;
    assert_eq!(
        resp.status(),
        403,
        "minting an upload token from a link must be refused, got {}",
        resp.status()
    );
}

/// `POST /api2/beshared-repos/{repo_id}/` must reject a permission level it does
/// not understand instead of silently storing it (read is granted for any
/// non-NULL value, so a typo became a read-share).
#[tokio::test]
async fn sharing_rejects_an_unknown_permission_level() {
    let server = TestServer::start().await;
    let client = server.client();
    common::create_test_user(&server.db, "owner@example.com", "password123").await;
    common::create_test_user(&server.db, "other@example.com", "password123").await;

    let login = client.login("owner@example.com", "password123").await;
    let token = login.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();
    let repo_id = common::create_test_repo(&client, &token, "lib").await;

    for permission in ["", "none", "read", "RW"] {
        let resp = client
            .post_json(
                &format!("/api2/beshared-repos/{repo_id}/"),
                Some(&token),
                &serde_json::json!({
                    "share_type": "personal",
                    "user": "other@example.com",
                    "permission": permission,
                }),
            )
            .await;
        assert_eq!(
            resp.status(),
            400,
            "permission {permission:?} must be rejected, got {}",
            resp.status()
        );
    }

    // A valid level still works.
    let resp = client
        .post_json(
            &format!("/api2/beshared-repos/{repo_id}/"),
            Some(&token),
            &serde_json::json!({
                "share_type": "personal",
                "user": "other@example.com",
                "permission": "r",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200, "a valid share must still work");
}

/// Only the owner may enumerate every member's email address. The endpoint used
/// `RepoPathWrite`, so any `rw` collaborator could harvest the member list even
/// though every mutating member operation is owner-only.
#[tokio::test]
async fn member_list_is_owner_only() {
    let server = TestServer::start().await;
    let client = server.client();
    common::create_test_user(&server.db, "owner@example.com", "password123").await;
    common::create_test_user(&server.db, "rw@example.com", "password123").await;

    let login = client.login("owner@example.com", "password123").await;
    let owner_token = login.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();
    let repo_id = common::create_test_repo(&client, &owner_token, "lib").await;

    let resp = client
        .post_json(
            &format!("/api2/beshared-repos/{repo_id}/"),
            Some(&owner_token),
            &serde_json::json!({
                "share_type": "personal",
                "user": "rw@example.com",
                "permission": "rw",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200, "sharing failed");

    let login = client.login("rw@example.com", "password123").await;
    let rw_token = login.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = client
        .get(&format!("/api2/beshared-repos/{repo_id}/"), Some(&rw_token))
        .await;
    assert_eq!(
        resp.status(),
        403,
        "an rw member must not enumerate every member, got {}",
        resp.status()
    );

    let resp = client
        .get(
            &format!("/api2/beshared-repos/{repo_id}/"),
            Some(&owner_token),
        )
        .await;
    assert_eq!(
        resp.status(),
        200,
        "the owner must still see the member list"
    );
}

/// Admin-only account creation through the API must honour the configured
/// password policy, like every other password-setting path does.
#[tokio::test]
async fn admin_api_registration_enforces_the_password_policy() {
    let server = TestServer::start().await;
    let client = server.client();
    common::create_test_admin(&server.db, "admin@example.com", "password123").await;

    let login = client.login("admin@example.com", "password123").await;
    let token = login.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = client
        .post_form(
            "/api2/accounts/",
            Some(&token),
            &[("email", "weak@example.com"), ("password", "x")],
        )
        .await;
    assert_eq!(
        resp.status(),
        400,
        "a one-character password must be refused, got {}",
        resp.status()
    );

    let resp = client
        .post_form(
            "/api2/accounts/",
            Some(&token),
            &[("email", "ok@example.com"), ("password", "goodpassword1")],
        )
        .await;
    assert_eq!(resp.status(), 200, "a compliant password must be accepted");
}
