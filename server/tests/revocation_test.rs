//! Credential-revocation regression tests.
//!
//! Each case here closes a window in which a credential kept working after the
//! event that was supposed to invalidate it: a deactivation, an unshare, a
//! device wipe, a password reset.

mod common;

use common::{TestFixture, create_test_user};
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

    // The fixture's token belongs to device "dev-1"; issue a fresh login on a
    // named device so the wipe can address it.
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

    // A sync token for the same device: a second user is not needed, the repo
    // token peer id is the client_id query parameter.
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

    // The device can use the repo token against /seafhttp/.
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

    // Android's shape: Authorization header, empty body, no token field.
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

    // The device's API token is gone.
    let resp = f.client.ping(&device_token).await;
    assert_eq!(resp.status(), 401, "wiped device token must be invalid");

    // ...and so is its repository sync token.
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

/// Unsharing a member must revoke their repository sync token: `check-blocks`
/// and `locked-files` derive the caller from it, and a token lives up to a year
/// (`sync_token_ttl_days`). Without this a removed member kept a block-existence
/// oracle for the library.
#[tokio::test]
async fn unshare_revokes_sync_token_and_closes_the_block_oracle() {
    let f = TestFixture::new().await;
    create_test_user(&f.server.db, "member@example.com", "password").await;

    // Share the library with the member (rw).
    let resp = f
        .client
        .post_json(
            &format!("/api2/beshared-repos/{}/", f.repo_id),
            Some(&f.api_token),
            &serde_json::json!({
                "share_type": "personal",
                "user": "member@example.com",
                "permission": "rw",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200, "sharing failed");

    let login = f.client.login("member@example.com", "password").await;
    let member_token = login.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    let repo_tokens = f
        .client
        .get(
            &format!("/api2/repo-tokens/?repos={}", f.repo_id),
            Some(&member_token),
        )
        .await;
    let member_sync = repo_tokens
        .json::<serde_json::Value>()
        .await
        .unwrap()
        .get(&f.repo_id)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .expect("member sync token");

    // The member can use check-blocks while they are a member.
    let resp = f
        .client
        .post_sync_json(
            &format!("/seafhttp/repo/{}/check-blocks/", f.repo_id),
            &member_sync,
            &serde_json::json!([infra::crypto::fs_id::sha1_hex(b"whatever")]),
        )
        .await;
    assert_eq!(resp.status(), 200, "check-blocks should work while shared");

    // The owner removes the member.
    let resp = f
        .client
        .delete_json(
            &format!("/api2/beshared-repos/{}/", f.repo_id),
            Some(&f.api_token),
            &serde_json::json!({"share_type": "personal", "user": "member@example.com"}),
        )
        .await;
    assert_eq!(resp.status(), 200, "unshare failed");

    // The sync token is gone, so check-blocks fails closed.
    let resp = f
        .client
        .post_sync_json(
            &format!("/seafhttp/repo/{}/check-blocks/", f.repo_id),
            &member_sync,
            &serde_json::json!([infra::crypto::fs_id::sha1_hex(b"whatever")]),
        )
        .await;
    assert_eq!(
        resp.status(),
        403,
        "a removed member's sync token must stop working, got {}",
        resp.status()
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

    // Whichever password won must be the one that works, and only one of them.
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

    // The link is spent for good.
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

    // Mint an in-memory upload token for the user, as `/upload-link/` would.
    let link = f
        .client
        .get(
            &format!("/api2/repos/{}/upload-link/?p=/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(link.status(), 200, "upload-link failed");
    let upload_url: String = link.json().await.unwrap();

    // Change the password through the browser flow.
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

    // The pre-rotation reset link is dead.
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

    // The pre-rotation in-memory upload URL is dead too.
    let resp = f
        .client
        .post_upload_chunk(&upload_url, "/", "x.bin", b"12345", 0, 5)
        .await;
    assert_ne!(
        resp.status(),
        200,
        "an upload URL minted before the password change must not still work"
    );

    // And the new password is the live one.
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

    // Sanity: the URL works while the account is active.
    let ok = f
        .client
        .post_upload_chunk(&upload_url, "/", "before.bin", b"abc", 0, 3)
        .await;
    assert!(
        ok.status() == 200 || ok.status() == 201,
        "the URL should work while active, got {}",
        ok.status()
    );

    // Deactivate the account directly.
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

    // The refusal must be the 2FA challenge, not an unrelated error.
    assert_eq!(
        second
            .headers()
            .get("X-Seafile-OTP")
            .and_then(|v| v.to_str().ok()),
        Some("required"),
        "the server must ask for a fresh code"
    );
}
