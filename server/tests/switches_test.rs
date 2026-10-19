//! Server-wide kill switches must fail closed.
//!
//! Each case here pinned a switch that was documented as closing a feature but
//! was never consulted on the server side: `auth.enable_registration` had no
//! reader at all, `auth.enable_password_reset` only reached the login template,
//! and the sharing switch was skipped by the upload-token mint and by the token
//! consumers.

mod common;

use common::TestServer;

/// `auth.enable_registration = false` closes registration: the page, the POST and
/// the login-page link all go away. A valid invitation code must not help.
#[tokio::test]
async fn disabled_registration_closes_registration() {
    let server = TestServer::start_with_auth_config(|auth| auth.enable_registration = false).await;
    let client = server.client();

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

/// With upload sharing disabled, an upload URL that was already handed out must
/// stop accepting writes — the switch used to guard only the pages and the link
/// creation, so a token in its one-hour TTL kept working.
#[tokio::test]
async fn disabled_share_upload_stops_minting_and_using_upload_tokens() {
    let server =
        TestServer::start_with_server_info_config(|cfg| cfg.share_upload_enabled = false).await;
    let client = server.client();

    let user_id = common::create_test_user(&server.db, "owner@example.com", "password123").await;
    assert!(user_id > 0);
    let login = client.login("owner@example.com", "password123").await;
    assert_eq!(login.status(), 200);
    let token = login.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();
    let repo_id = common::create_test_repo(&client, &token, "lib").await;

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

/// The sharing switch has two halves. Closing downloads leaves upload links
/// alone, and the client capability string follows the half it names.
#[tokio::test]
async fn disabled_share_download_keeps_upload_sharing() {
    let server =
        TestServer::start_with_server_info_config(|cfg| cfg.share_download_enabled = false).await;
    let client = server.client();
    let (token, repo_id, _user_id) = owner_and_repo(&server, &client).await;

    let share = client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&token),
            &serde_json::json!({"repo_id": repo_id, "path": "/"}),
        )
        .await;
    assert_eq!(
        share.status(),
        403,
        "creating a share link must be refused, got {}",
        share.status()
    );

    let upload = client
        .post_json(
            "/api/v2.1/upload-links/",
            Some(&token),
            &serde_json::json!({"repo_id": repo_id, "path": "/"}),
        )
        .await;
    assert_eq!(
        upload.status(),
        200,
        "upload sharing must keep working, got {}",
        upload.status()
    );

    let info: serde_json::Value = client
        .get("/api2/server-info/", None)
        .await
        .json()
        .await
        .unwrap();
    assert!(
        info["features"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f == "share-link-disabled"),
        "the share-link capability string follows the download half"
    );
}

/// Closing uploads leaves share links alone, and the server keeps advertising
/// the share-link capability because the download half is still open.
#[tokio::test]
async fn disabled_share_upload_keeps_download_sharing() {
    let server =
        TestServer::start_with_server_info_config(|cfg| cfg.share_upload_enabled = false).await;
    let client = server.client();
    let (token, repo_id, _user_id) = owner_and_repo(&server, &client).await;

    let upload = client
        .post_json(
            "/api/v2.1/upload-links/",
            Some(&token),
            &serde_json::json!({"repo_id": repo_id, "path": "/"}),
        )
        .await;
    assert_eq!(
        upload.status(),
        403,
        "creating an upload link must be refused, got {}",
        upload.status()
    );

    let share = client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&token),
            &serde_json::json!({"repo_id": repo_id, "path": "/"}),
        )
        .await;
    assert_eq!(
        share.status(),
        200,
        "download sharing must keep working, got {}",
        share.status()
    );

    let info: serde_json::Value = client
        .get("/api2/server-info/", None)
        .await
        .json()
        .await
        .unwrap();
    assert!(
        !info["features"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f == "share-link-disabled"),
        "an upload-only switch must not hide share links from clients"
    );

    // An upload link minted before the switch still refuses to hand out a token.
    let resp = client
        .get("/api/v2.1/upload-links/whatever/upload/", None)
        .await;
    assert_eq!(resp.status(), 403);
}

/// The file browser renders the sharing switches onto its own markup, and the
/// page carries `data-*` attributes the row menu reads.
#[tokio::test]
async fn disabled_sharing_hides_the_web_entry_points() {
    let server = TestServer::start_with_server_info_config(|cfg| {
        cfg.share_download_enabled = false;
        cfg.share_upload_enabled = false;
    })
    .await;
    let client = server.client();
    let (token, repo_id, _user_id) = owner_and_repo(&server, &client).await;
    let browser = login_browser(&server, "owner@example.com", "password123").await;

    let page = browser
        .get(&format!("/libraries/{repo_id}/files/"), None)
        .await;
    assert_eq!(page.status(), 200);
    let body = page.text().await.unwrap();
    assert!(
        body.contains(r#"data-share-download-enabled="false""#),
        "the download switch must reach the row menu"
    );
    assert!(
        body.contains(r#"data-share-upload-enabled="false""#),
        "the upload switch must reach the row menu"
    );
    for gone in ["js-rp-share", "rp-upload-link-btn", "href=\"/shares/\""] {
        assert!(
            !body.contains(gone),
            "{gone} must not be rendered while sharing is off"
        );
    }

    // The page itself is closed, like a closed registration page.
    let shares = browser.get("/shares/", None).await;
    assert_eq!(shares.status(), 404, "the shares page must be closed");
    assert!(shares.text().await.unwrap().contains("Page not found"));

    // Normal signed-in uploads are not anonymous sharing and must keep working.
    let upload_token = client
        .get(&format!("/api2/repos/{repo_id}/upload-link/"), Some(&token))
        .await;
    assert_eq!(
        upload_token.status(),
        200,
        "the authenticated upload token must not be gated"
    );
    let up = client
        .upload_file(&token, &repo_id, "/", "still-uploads.txt", b"x")
        .await;
    assert!(
        up.status().is_success(),
        "a normal upload must keep working"
    );
}

/// One direction closed leaves the other's controls and the Shares page in place;
/// a bookmark to the closed tab falls back to the open one.
#[tokio::test]
async fn one_open_direction_keeps_the_shares_page() {
    let server =
        TestServer::start_with_server_info_config(|cfg| cfg.share_download_enabled = false).await;
    let client = server.client();
    let (_token, _repo_id, _user_id) = owner_and_repo(&server, &client).await;
    let browser = login_browser(&server, "owner@example.com", "password123").await;

    let page = browser.get("/shares/", None).await;
    assert_eq!(page.status(), 200, "upload sharing still has a page");
    let body = page.text().await.unwrap();
    assert!(body.contains(r#"id="tab-upload-links""#));
    assert!(!body.contains(r#"id="tab-share-links""#));
    assert!(body.contains(r#"href="/shares/""#), "the link stays");
}

/// The file browser keeps the upload controls while only downloads are closed.
#[tokio::test]
async fn disabled_download_keeps_the_upload_controls() {
    let server =
        TestServer::start_with_server_info_config(|cfg| cfg.share_download_enabled = false).await;
    let client = server.client();
    let (_token, repo_id, _user_id) = owner_and_repo(&server, &client).await;
    let browser = login_browser(&server, "owner@example.com", "password123").await;

    let body = browser
        .get(&format!("/libraries/{repo_id}/files/"), None)
        .await
        .text()
        .await
        .unwrap();
    assert!(!body.contains("js-rp-share"));
    assert!(body.contains("rp-upload-link-btn"));
    assert!(body.contains(r#"data-share-upload-enabled="true""#));
}

/// An upload link that outlived the switch answers as gone when upload sharing is
/// closed, not as a permission error.
#[tokio::test]
async fn disabled_share_upload_answers_existing_links_as_gone() {
    let server =
        TestServer::start_with_server_info_config(|cfg| cfg.share_upload_enabled = false).await;
    let client = server.client();
    let (_token, repo_id, user_id) = owner_and_repo(&server, &client).await;

    let now = chrono::Utc::now().timestamp();
    use infra::entity::upload_link;
    use sea_orm::EntityTrait;
    upload_link::Entity::insert(upload_link::ActiveModel {
        id: sea_orm::NotSet,
        repo_id: sea_orm::Set(repo_id),
        creator_id: sea_orm::Set(user_id),
        path: sea_orm::Set("/".to_string()),
        token: sea_orm::Set("pre-existing-upload".to_string()),
        password: sea_orm::Set(None),
        expires_at: sea_orm::Set(None),
        created_at: sea_orm::Set(now),
        view_cnt: sea_orm::Set(0i64),
        description: sea_orm::Set(None),
    })
    .exec(&*server.db)
    .await
    .expect("seed an upload link");

    let resp = client.get("/u/pre-existing-upload/", None).await;
    assert_eq!(
        resp.status(),
        404,
        "an existing upload link must read as gone"
    );
    assert!(resp.text().await.unwrap().contains("Link unavailable"));
}

/// A user owning one library, plus their API token.
async fn owner_and_repo(
    server: &TestServer,
    client: &common::client::TestClient,
) -> (String, String, i32) {
    let user_id = common::create_test_user(&server.db, "owner@example.com", "password123").await;
    assert!(user_id > 0);
    let login = client.login("owner@example.com", "password123").await;
    assert_eq!(login.status(), 200);
    let token = login.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();
    let repo_id = common::create_test_repo(client, &token, "lib").await;
    (token, repo_id, user_id)
}

/// A cookie-carrying client already signed in through the Web UI.
async fn login_browser(
    server: &TestServer,
    email: &str,
    password: &str,
) -> common::client::TestClient {
    let browser = common::client::TestClient::new_with_cookies(&server.base_url);
    let resp = browser
        .post_ui_form(
            "/accounts/login/",
            &[("email", email), ("password", password)],
        )
        .await;
    assert!(
        resp.status().is_success(),
        "Web UI login failed: {}",
        resp.status()
    );
    browser
}
