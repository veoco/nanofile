//! Credential-revocation regression tests.
//!
//! Each case here closes a window in which a credential kept working after the
//! event that was supposed to invalidate it: a deactivation, a device wipe, a
//! password reset.

use crate::common;

use common::TestFixture;
use sea_orm::{ActiveModelTrait, EntityTrait, Set};

/// Set a short-lived access token aside by writing it directly, returning the
/// raw value. Used to prove that revocation really deletes rows.
async fn seed_reset_token(f: &TestFixture, raw: &str) -> i32 {
    let now = chrono::Utc::now().timestamp();
    let hash = server::service::auth::password_reset::hash_token(raw);
    let model = f
        .server
        .repos
        .password_reset_token
        .create(f.user_id, hash, now, now + 3600)
        .await
        .expect("create reset token");
    model.id
}

/// `/api2/device-wiped/` must accept the two shapes the official clients send:
/// a `token` form field with no bearer (desktop Qt) **and** an empty body with
/// an `Authorization: Token` header (Android). Revoking only the API token left
/// the device's repository sync token — valid for `sync_token_ttl_days`, a year
/// by default — able to keep syncing.
#[tokio::test]
async fn device_wiped_accepts_bearer_and_revokes_sync_tokens() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .post_form(
            "/api2/auth-token/",
            None,
            &[
                ("username", &f.email),
                ("password", &f.password),
                ("platform", "android"),
                ("device_id", "android-dev"),
                ("device_name", "phone"),
                ("client_version", "3.0.0"),
            ],
        )
        .await;
    assert_eq!(resp.status(), 200, "login with a device id failed");
    let device_token = resp.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    let repo_tokens = f
        .client
        .get(
            &format!("/api2/repo-tokens/?repos={}", f.repo_id),
            Some(&device_token),
        )
        .await;
    assert_eq!(repo_tokens.status(), 200, "repo-tokens failed");
    let sync_token = repo_tokens
        .json::<serde_json::Value>()
        .await
        .unwrap()
        .get(&f.repo_id)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .expect("sync token for the repo");

    let probe = f
        .client
        .put_sync(
            &format!(
                "/seafhttp/repo/{}/block/{}?client_id=android-dev",
                f.repo_id,
                infra::crypto::fs_id::sha1_hex(b"probe-block")
            ),
            &sync_token,
            b"probe-block".to_vec(),
        )
        .await;
    assert!(
        probe.status() == 200 || probe.status() == 443 || probe.status() == 446,
        "the sync token should be accepted before the wipe, got {}",
        probe.status()
    );

    let resp = f
        .client
        .post_bytes("/api2/device-wiped/", Some(&device_token), Vec::new())
        .await;
    assert_eq!(
        resp.status(),
        200,
        "bearer-only wipe report must be accepted (Android), got: {}",
        resp.text().await.unwrap_or_default()
    );

    let resp = f.client.ping(&device_token).await;
    assert_eq!(resp.status(), 401, "wiped device token must be invalid");

    let probe = f
        .client
        .put_sync(
            &format!(
                "/seafhttp/repo/{}/block/{}?client_id=android-dev",
                f.repo_id,
                infra::crypto::fs_id::sha1_hex(b"probe-block-2")
            ),
            &sync_token,
            b"probe-block-2".to_vec(),
        )
        .await;
    assert_eq!(
        probe.status(),
        403,
        "the wiped device's sync token must stop working, got {}",
        probe.status()
    );
}

/// A password reset link is single use even when two submissions race: the
/// token is claimed with one conditional UPDATE before the password is written,
/// so the loser cannot overwrite the winner's new password.
#[tokio::test]
async fn password_reset_token_is_single_use_under_concurrency() {
    let f = TestFixture::new().await;

    let raw = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
    seed_reset_token(&f, raw).await;

    let base = f.server.base_url.clone();
    let url = format!("{base}/accounts/password/reset/{raw}/");

    // Two concurrent submissions, each with a different new password. A bare
    // non-cookie client needs no CSRF token; the Origin check passes when the
    // header is absent.
    // No redirect following: a successful reset answers 302 (to the "complete"
    // page) while a rejected one renders the error page with 200, so following
    // the redirect would erase the distinction.
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let first = client
        .post(&url)
        .header("origin", &base)
        .form(&[
            ("password1", "firstpassword1"),
            ("password2", "firstpassword1"),
        ])
        .send();
    let second = client
        .post(&url)
        .header("origin", &base)
        .form(&[
            ("password1", "secondpassword2"),
            ("password2", "secondpassword2"),
        ])
        .send();
    let (a, b) = tokio::join!(first, second);
    let (a, b) = (a.unwrap(), b.unwrap());

    // Exactly one submission may take effect: the other must be rejected (the
    // confirm page with an error, HTTP 200) rather than changing the password
    // again.
    let statuses = [a.status().as_u16(), b.status().as_u16()];
    let winners = statuses.iter().filter(|s| **s == 302).count();
    assert_eq!(
        winners, 1,
        "exactly one concurrent reset may succeed, got {statuses:?}"
    );

    let first_ok = f
        .client
        .login(&f.email, "firstpassword1")
        .await
        .status()
        .as_u16()
        == 200;
    let second_ok = f
        .client
        .login(&f.email, "secondpassword2")
        .await
        .status()
        .as_u16()
        == 200;
    assert_ne!(
        first_ok, second_ok,
        "exactly one of the two new passwords must be accepted"
    );

    let resp = client
        .post(&url)
        .header("origin", &base)
        .form(&[
            ("password1", "thirdpassword3"),
            ("password2", "thirdpassword3"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "a spent link must not reset again");
}

/// Changing the password must also delete any outstanding reset token and the
/// user's in-memory capability URLs, so a leaked link or upload/download URL
/// cannot outlive the remediation. Driven through the settings form (cookie +
/// CSRF), which is the real self-service path.
#[tokio::test]
async fn password_change_revokes_reset_tokens_and_access_tokens() {
    let f = TestFixture::new().await;

    let raw = "feedfacefeedfacefeedfacefeedfacefeedfacefeedfacefeedfacefeedface";
    seed_reset_token(&f, raw).await;

    let link = f
        .client
        .get(
            &format!("/api2/repos/{}/upload-link/?p=/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(link.status(), 200, "upload-link failed");
    let upload_url: String = link.json().await.unwrap();

    let cookie_client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .cookie_store(true)
        .build()
        .unwrap();
    let base = f.server.base_url.clone();
    let resp = cookie_client
        .post(format!("{base}/accounts/login/"))
        .form(&[("email", &f.email), ("password", &f.password)])
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_redirection(), "browser login failed");

    let settings = cookie_client
        .get(format!("{base}/settings/"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let csrf = settings
        .split(r#"name="csrf_token" value=""#)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_default()
        .to_string();
    assert!(!csrf.is_empty(), "settings page must expose a CSRF token");

    let resp = cookie_client
        .post(format!("{base}/settings/password/"))
        .form(&[
            ("old_password", f.password.as_str()),
            ("new_password", "rotatedpass1"),
            ("csrf_token", csrf.as_str()),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 302, "password change should redirect");

    let model = f
        .server
        .repos
        .password_reset_token
        .find_by_token_hash(&server::service::auth::password_reset::hash_token(raw))
        .await
        .unwrap();
    assert!(
        model.is_none(),
        "a password change must delete outstanding reset tokens"
    );

    let resp = f
        .client
        .post_upload_chunk(&upload_url, "/", "x.bin", b"12345", 0, 5)
        .await;
    assert_ne!(
        resp.status(),
        200,
        "an upload URL minted before the password change must not still work"
    );

    let resp = f.client.login(&f.email, "rotatedpass1").await;
    assert_eq!(resp.status(), 200, "new password must work");
}

/// `AuthUser` rejects a deactivated account, and the token-only endpoints
/// (`/upload-api/…`, `/download-api/…`) now do too — they authenticate with an
/// in-memory capability instead of a session, so they never saw that check.
#[tokio::test]
async fn deactivating_an_account_stops_its_capability_urls() {
    let f = TestFixture::new().await;

    let link = f
        .client
        .get(
            &format!("/api2/repos/{}/upload-link/?p=/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    let upload_url: String = link.json().await.unwrap();

    let ok = f
        .client
        .post_upload_chunk(&upload_url, "/", "before.bin", b"abc", 0, 3)
        .await;
    assert!(
        ok.status() == 200 || ok.status() == 201,
        "the URL should work while active, got {}",
        ok.status()
    );

    let user = infra::entity::user::Entity::find_by_id(f.user_id)
        .one(&*f.server.db)
        .await
        .unwrap()
        .unwrap();
    let mut active: infra::entity::user::ActiveModel = user.into();
    active.is_active = Set(false);
    active.update(&*f.server.db).await.unwrap();

    let resp = f
        .client
        .post_upload_chunk(&upload_url, "/", "after.bin", b"xyz", 0, 3)
        .await;
    assert_eq!(
        resp.status(),
        403,
        "a deactivated account's capability URL must stop working, got {}",
        resp.status()
    );
}

/// A TOTP code is single-use: the time step is claimed with one conditional
/// UPDATE, so replaying an observed code inside the ±1-step skew window fails.
/// The guard used to read the stored step and then write it, which left a
/// window where two submissions of the same code both succeeded — and it also
/// accepted the code when the write failed.
#[tokio::test]
async fn totp_code_cannot_be_replayed() {
    let f = TestFixture::new().await;

    let secret = server::service::auth::totp::TotpManager::generate_secret();
    f.server
        .repos
        .user_2fa
        .get_or_create(f.user_id, secret.clone())
        .await
        .expect("store the TOTP seed");
    f.server
        .repos
        .user_2fa
        .set_enabled(f.user_id, true, chrono::Utc::now().timestamp())
        .await
        .expect("enable 2FA");

    let totp = server::service::auth::totp::TotpManager::create_totp(&secret, &f.email, "Nanofile")
        .expect("build TOTP");
    let code = totp.generate_current().to_string();

    let first = f.client.login_with_otp(&f.email, &f.password, &code).await;
    assert_eq!(
        first.status(),
        200,
        "the current code must be accepted once, got: {}",
        first.status()
    );

    let second = f.client.login_with_otp(&f.email, &f.password, &code).await;
    assert_eq!(
        second.status(),
        400,
        "a replayed code must be refused, got: {}",
        second.status()
    );

    assert_eq!(
        second
            .headers()
            .get("X-Seafile-OTP")
            .and_then(|v| v.to_str().ok()),
        Some("required"),
        "the server must ask for a fresh code"
    );
}

/// A block-upload part whose filename is the block's SHA-1, as the block API
/// requires.
fn block_form(data: &[u8]) -> reqwest::multipart::Form {
    use sha1::{Digest, Sha1};
    let mut hasher = Sha1::new();
    hasher.update(data);
    let block_id = hex::encode(hasher.finalize());
    let part = reqwest::multipart::Part::bytes(data.to_vec())
        .file_name(block_id)
        .mime_str("application/octet-stream")
        .unwrap();
    reqwest::multipart::Form::new().part("file", part)
}

/// Deactivation must stop every capability URL, including the block-level ones:
/// `/upload-blks-api` and `/blks/` authenticate with an in-memory token rather
/// than a session, so they need the same `is_active` check as `/download-api`.
#[tokio::test]
async fn deactivating_an_account_stops_block_capability_urls() {
    let f = TestFixture::new().await;

    let link = f.client.upload_blks_link(&f.api_token, &f.repo_id).await;
    assert_eq!(link.status(), 200);
    let upload_url: String = link.json().await.unwrap();

    // A committed file gives us a block-download URL to test as well.
    assert!(
        f.client
            .upload_file(&f.api_token, &f.repo_id, "/", "block.bin", b"block-bytes")
            .await
            .status()
            .is_success()
    );
    let info = f
        .client
        .get(
            &format!(
                "/api2/repos/{}/file/?p=/block.bin&op=downloadblks",
                f.repo_id
            ),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(info.status(), 200);
    let info: serde_json::Value = info.json().await.unwrap();
    let file_id = info["file_id"].as_str().expect("file id");
    let block_id = info["blklist"][0].as_str().expect("block id");
    let link = f
        .client
        .get(
            &format!(
                "/api2/repos/{}/files/{file_id}/blks/{block_id}/download-link/",
                f.repo_id
            ),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(link.status(), 200);
    let block_url: String = link.json().await.unwrap();
    let block_path = block_url
        .strip_prefix(&f.server.base_url)
        .expect("block URL uses the server base");

    // The upload URL works while the account is active.
    let resp = f
        .client
        .post_multipart_url(&upload_url, block_form(b"before"))
        .await;
    assert_eq!(resp.status(), 200);

    let user = infra::entity::user::Entity::find_by_id(f.user_id)
        .one(&*f.server.db)
        .await
        .unwrap()
        .unwrap();
    let mut active: infra::entity::user::ActiveModel = user.into();
    active.is_active = Set(false);
    active.update(&*f.server.db).await.unwrap();

    let resp = f
        .client
        .post_multipart_url(&upload_url, block_form(b"after"))
        .await;
    assert_eq!(
        resp.status(),
        403,
        "a deactivated account's block-upload URL must stop working"
    );

    let resp = f.client.get(block_path, None).await;
    assert_eq!(
        resp.status(),
        403,
        "a deactivated account's block-download URL must stop working"
    );
}

/// Deactivation revokes the account's outstanding capability URLs outright, not
/// only when they happen to hit an endpoint that re-checks `is_active`.
#[tokio::test]
async fn deactivating_an_account_revokes_outstanding_capability_tokens() {
    let f = TestFixture::new().await;

    let link = f.client.upload_blks_link(&f.api_token, &f.repo_id).await;
    assert_eq!(link.status(), 200);
    let upload_url: String = link.json().await.unwrap();
    let token = upload_url
        .rsplit('/')
        .next()
        .expect("token in the block URL")
        .to_string();
    assert!(
        f.server.state.token_manager.validate(&token).is_some(),
        "the token must be live before deactivation"
    );

    let user = infra::entity::user::Entity::find_by_id(f.user_id)
        .one(&*f.server.db)
        .await
        .unwrap()
        .unwrap();
    let is_admin = user.is_admin;
    let storage_quota = user.storage_quota;

    let svc = server::service::admin::AdminUserService::new(f.server.repos.clone())
        .with_token_manager(f.server.state.token_manager.clone());
    svc.update_user(f.user_id, is_admin, false, storage_quota)
        .await
        .unwrap();

    assert!(
        f.server.state.token_manager.validate(&token).is_none(),
        "deactivation must revoke outstanding capability URLs"
    );
}

/// Log in through the web form and return the cookie-holding browser plus the
/// CSRF token the settings pages embed.
async fn web_session(f: &TestFixture) -> (reqwest::Client, String) {
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .cookie_store(true)
        .build()
        .unwrap();
    let resp = client
        .post(format!("{}/accounts/login/", f.server.base_url))
        .form(&[
            ("email", f.email.as_str()),
            ("password", f.password.as_str()),
        ])
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_redirection(),
        "web login failed: {}",
        resp.status()
    );
    let csrf = settings_csrf_token(&client, &f.server.base_url).await;
    assert!(!csrf.is_empty(), "settings page must expose a CSRF token");
    (client, csrf)
}

/// Scrape the `csrf_token` hidden input from the settings page.
async fn settings_csrf_token(client: &reqwest::Client, base: &str) -> String {
    let body = client
        .get(format!("{base}/settings/"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    body.split(r#"name="csrf_token" value=""#)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_default()
        .to_string()
}

/// Seed a 2FA device-trust token ("skip 2FA on this device") for the fixture user.
async fn seed_s2fa_token(f: &TestFixture, raw: &str) {
    let now = chrono::Utc::now().timestamp();
    f.server
        .repos
        .s2fa_token
        .create_s2fa_token(server::repository::s2fa_token::CreateS2faTokenParams {
            user_id: f.user_id,
            token: raw.to_string(),
            device_id: Some("trusted-device".to_string()),
            device_name: Some("laptop".to_string()),
            created_at: now,
            expires_at: now + 3600,
        })
        .await
        .expect("create a device-trust token");
}

/// Enabling 2FA is a security boundary: the verify step drops every
/// device-trust token (proof of the *old* second factor, which must not count as
/// proof of the new one) and revokes the account's other credentials, while the
/// session that performed the change must survive.
#[tokio::test]
async fn verifying_2fa_revokes_other_credentials_and_keeps_the_session() {
    let f = TestFixture::new().await;

    // A pending setup, as if the user had just clicked "Set Up".
    let secret = server::service::auth::totp::TotpManager::generate_secret();
    f.server
        .repos
        .user_2fa
        .get_or_create(f.user_id, secret.clone())
        .await
        .expect("seed a pending 2FA setup");
    seed_s2fa_token(&f, "stale-device-trust").await;

    assert_eq!(
        f.client.ping(&f.api_token).await.status(),
        200,
        "the fixture's API token works before verification"
    );

    let (client, csrf) = web_session(&f).await;
    let totp = server::service::auth::totp::TotpManager::create_totp(&secret, &f.email, "Nanofile")
        .expect("build TOTP");
    let code = totp.generate_current().to_string();

    let resp = client
        .post(format!("{}/settings/two-factor/verify/", f.server.base_url))
        .form(&[("code", code.as_str()), ("csrf_token", csrf.as_str())])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "verifying the code enables 2FA");

    assert!(
        f.server
            .repos
            .user_2fa
            .find_by_user_id(f.user_id)
            .await
            .unwrap()
            .expect("2FA row")
            .enabled,
        "2FA must be enabled after verification"
    );
    assert!(
        f.server
            .repos
            .s2fa_token
            .list_for_user(f.user_id)
            .await
            .unwrap()
            .is_empty(),
        "enabling 2FA must drop every device-trust token"
    );

    assert_eq!(
        client
            .get(format!("{}/settings/", f.server.base_url))
            .send()
            .await
            .unwrap()
            .status(),
        200,
        "the session that enabled 2FA must stay usable"
    );
    assert_eq!(
        f.client.ping(&f.api_token).await.status(),
        401,
        "other API tokens must be revoked by enabling 2FA"
    );
}

/// Disabling 2FA must drop the device-trust tokens too: they are proof of a
/// second factor that no longer exists, and a stolen one would otherwise let a
/// device skip a future setup's verification.
#[tokio::test]
async fn disabling_2fa_drops_device_trusts_and_keeps_the_session() {
    let f = TestFixture::new().await;

    // Log in *before* 2FA is on: a 2FA-enabled login stops at the pending
    // session / code page, and the disable form needs a real session.
    let (client, csrf) = web_session(&f).await;

    let secret = server::service::auth::totp::TotpManager::generate_secret();
    f.server
        .repos
        .user_2fa
        .get_or_create(f.user_id, secret)
        .await
        .expect("seed a 2FA row");
    f.server
        .repos
        .user_2fa
        .set_enabled(f.user_id, true, chrono::Utc::now().timestamp())
        .await
        .expect("enable 2FA");
    seed_s2fa_token(&f, "device-trust").await;

    let resp = client
        .post(format!(
            "{}/settings/two-factor/disable/",
            f.server.base_url
        ))
        .form(&[
            ("password", f.password.as_str()),
            ("csrf_token", csrf.as_str()),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 302, "the correct password disables 2FA");

    assert!(
        f.server
            .repos
            .user_2fa
            .find_by_user_id(f.user_id)
            .await
            .unwrap()
            .is_none(),
        "the 2FA row must be removed"
    );
    assert!(
        f.server
            .repos
            .s2fa_token
            .list_for_user(f.user_id)
            .await
            .unwrap()
            .is_empty(),
        "disabling 2FA must drop every device-trust token"
    );
    assert_eq!(
        client
            .get(format!("{}/settings/", f.server.base_url))
            .send()
            .await
            .unwrap()
            .status(),
        200,
        "disabling 2FA must not log the acting browser out"
    );
}

/// Deactivation is a revocation, not just a flag: the rows are deleted so that
/// re-enabling the account cannot resurrect an old session, sync token or API
/// key, and an unused reset link cannot outlive the deactivation.
#[tokio::test]
async fn deactivation_revokes_credentials_and_reset_links() {
    let f = TestFixture::new().await;
    let raw_reset = "reset-token-for-deactivation";
    let _ = seed_reset_token(&f, raw_reset).await;
    let reset =
        server::service::auth::password_reset::PasswordResetService::new(f.server.repos.clone());
    assert!(
        reset.validate_token(raw_reset).await.unwrap().is_some(),
        "the reset token must be live before deactivation"
    );

    f.server
        .state
        .admin_user_service()
        .update_user(f.user_id, false, false, None)
        .await
        .expect("deactivate the account");

    assert!(
        f.server
            .repos
            .api_token
            .find_by_token(&f.api_token)
            .await
            .unwrap()
            .is_none(),
        "the account token must be deleted, not only blocked"
    );
    assert!(
        f.server
            .repos
            .sync_token
            .find_by_token(&f.sync_token)
            .await
            .unwrap()
            .is_none(),
        "the repository sync token must be deleted"
    );
    assert!(
        f.server
            .repos
            .password_reset_token
            .find_by_user(f.user_id)
            .await
            .unwrap()
            .is_empty(),
        "unused reset links must be deleted"
    );
    assert!(
        reset.validate_token(raw_reset).await.unwrap().is_none(),
        "the old reset link must stop working"
    );

    let denied = f
        .client
        .get("/api2/account/info/", Some(&f.api_token))
        .await;
    assert_eq!(
        denied.status(),
        401,
        "the deleted token must not authenticate"
    );

    f.server
        .state
        .admin_user_service()
        .update_user(f.user_id, false, true, None)
        .await
        .expect("reactivate the account");
    assert!(
        f.server
            .repos
            .api_token
            .find_by_token(&f.api_token)
            .await
            .unwrap()
            .is_none(),
        "re-enabling the account must not restore the old token"
    );
    assert!(
        reset.validate_token(raw_reset).await.unwrap().is_none(),
        "re-enabling the account must not restore the reset link"
    );
}

/// A changed master secret revokes every stored session at the next start. The
/// first start on an existing database only records the fingerprint, so upgrading
/// does not log everybody out.
#[tokio::test]
async fn a_changed_master_secret_revokes_sessions_at_startup() {
    let f = TestFixture::new().await;
    let db = f.server.db.as_ref();

    assert_eq!(
        server::service::auth::session_fingerprint::enforce(db, "secret-one")
            .await
            .unwrap(),
        0,
        "the first start must only record the fingerprint"
    );
    let after_first = server::repository::Repositories::without_secrets(f.server.db.clone());
    assert!(
        after_first
            .api_token
            .find_by_token(&f.api_token)
            .await
            .unwrap()
            .is_some(),
        "recording the fingerprint must not revoke an existing session"
    );

    let revoked = server::service::auth::session_fingerprint::enforce(db, "secret-two")
        .await
        .unwrap();
    assert!(
        revoked >= 1,
        "a changed secret must delete the session rows"
    );
    let after_change = server::repository::Repositories::without_secrets(f.server.db.clone());
    assert!(
        after_change
            .api_token
            .find_by_token(&f.api_token)
            .await
            .unwrap()
            .is_none(),
        "the old session token must be gone from the database"
    );

    assert_eq!(
        server::service::auth::session_fingerprint::enforce(db, "secret-two")
            .await
            .unwrap(),
        0,
        "the same secret must not revoke the sessions again"
    );
}
