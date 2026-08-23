mod common;

use common::{TestServer, create_test_user};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};
use server::service::auth::backup_codes::BackupCodeManager;

/// Security: spoofing X-Forwarded-For must NOT bypass the per-IP login rate
/// limit. The limiter keys on the TCP peer address, so rotating the header
/// across failed attempts still accumulates failures on the real client IP.
#[tokio::test]
async fn test_login_rate_limit_not_bypassable_via_xff_spoofing() {
    let server = TestServer::start().await;
    create_test_user(server.db.as_ref(), "victim@example.com", "secret123").await;

    // Raw client so we can attach arbitrary X-Forwarded-For headers.
    let raw = reqwest::Client::builder().no_proxy().build().unwrap();
    let url = format!("{}/api2/auth-token/", server.base_url);

    // 5 failed attempts: DIFFERENT usernames, DIFFERENT spoofed XFF values.
    for i in 0..5 {
        let resp = raw
            .post(&url)
            .header("X-Forwarded-For", format!("10.0.0.{i}"))
            .form(&[
                ("username", format!("ghost{i}@example.com")),
                ("password", "wrong".to_string()),
            ])
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 400, "failed attempt {i} should be rejected");
    }

    // 6th attempt with CORRECT credentials and yet another spoofed XFF. It
    // must be rate-limited (not 200) because all 5 failures share the socket IP.
    let resp = raw
        .post(&url)
        .header("X-Forwarded-For", "10.0.0.99")
        .form(&[
            ("username", "victim@example.com"),
            ("password", "secret123"),
        ])
        .send()
        .await
        .unwrap();
    assert_ne!(
        resp.status(),
        200,
        "XFF spoofing must not bypass the per-IP login rate limit"
    );
}

#[tokio::test]
async fn test_login_success() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;

    let resp = client.login("test@example.com", "password123").await;
    let status = resp.status();
    let body_text = resp.text().await.unwrap_or_default();
    eprintln!("STATUS: {}, BODY: {}", status, body_text);
    assert_eq!(status, 200, "response body: {}", body_text);

    let body: serde_json::Value = serde_json::from_str(&body_text).unwrap();
    let token = body["token"].as_str().unwrap();
    assert_eq!(token.len(), 40);
}

/// Security: the anonymous login endpoint must cap request-body size so an
/// attacker cannot force a huge in-memory buffer (the global upload limit is
/// 4 GiB). The 1 MiB small-body cap must yield 413.
#[tokio::test]
async fn test_login_body_size_limited() {
    let server = TestServer::start().await;
    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;

    let raw = reqwest::Client::builder().no_proxy().build().unwrap();
    let url = format!("{}/api2/auth-token/", server.base_url);

    // 2 MiB of form data: over the 1 MiB cap, far under the 4 GiB global limit.
    let mut body = String::from("username=test@example.com&password=password123&");
    body.push_str(&"x".repeat(2 * 1024 * 1024));

    let resp = raw.post(&url).body(body).send().await.unwrap();
    assert_eq!(
        resp.status(),
        413,
        "oversized login body must be rejected with 413"
    );
}

#[tokio::test]
async fn test_login_wrong_password() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;

    let resp = client.login("test@example.com", "wrongpassword").await;
    assert_eq!(resp.status(), 400);

    let body: serde_json::Value = resp.json().await.unwrap();
    let errors = body["non_field_errors"].as_array().unwrap();
    assert!(
        errors
            .iter()
            .any(|e| e.as_str().unwrap().contains("Unable to login")),
        "expected 'Unable to login' error, got: {:?}",
        body
    );
}

#[tokio::test]
async fn test_login_nonexistent_user() {
    let server = TestServer::start().await;
    let client = server.client();

    let resp = client.login("nonexistent@example.com", "password123").await;
    assert_eq!(resp.status(), 400);

    let body: serde_json::Value = resp.json().await.unwrap();
    let errors = body["non_field_errors"].as_array().unwrap();
    assert!(
        errors
            .iter()
            .any(|e| e.as_str().unwrap().contains("Unable to login")),
        "expected 'Unable to login' error, got: {:?}",
        body
    );
}

#[tokio::test]
async fn test_login_success_json() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;

    // Login with JSON body
    let resp = client.login_json("test@example.com", "password123").await;
    assert_eq!(resp.status(), 200, "JSON login should succeed");

    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();
    assert_eq!(token.len(), 40);
}

#[tokio::test]
async fn test_login_wrong_password_json() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;

    let resp = client.login_json("test@example.com", "wrongpassword").await;
    assert_eq!(resp.status(), 400);

    let body: serde_json::Value = resp.json().await.unwrap();
    let errors = body["non_field_errors"].as_array().unwrap();
    assert!(
        errors
            .iter()
            .any(|e| e.as_str().unwrap().contains("Unable to login")),
        "expected 'Unable to login' error, got: {:?}",
        body
    );
}

#[tokio::test]
async fn test_login_2fa_required_no_otp() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;

    // Manually enable 2FA for the user
    let user_2fa = infra::entity::user_2fa::ActiveModel {
        user_id: sea_orm::Set(1),
        totp_secret: sea_orm::Set("JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP".to_string()),
        algorithm: sea_orm::Set("SHA1".to_string()),
        digits: sea_orm::Set(6),
        period: sea_orm::Set(30),
        enabled: sea_orm::Set(true),
        enabled_at: sea_orm::NotSet,
    };
    user_2fa.insert(server.db.as_ref()).await.unwrap();

    let resp = client.login("test@example.com", "password123").await;
    assert_eq!(resp.status(), 400);

    let body: serde_json::Value = resp.json().await.unwrap();
    let errors = body["non_field_errors"].as_array().unwrap();
    assert!(
        errors.iter().any(|e| e
            .as_str()
            .unwrap()
            .contains("Two factor auth token is missing")),
        "expected 2FA error message, got: {:?}",
        body
    );
}

#[tokio::test]
async fn test_login_2fa_invalid_otp() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;

    let user_2fa = infra::entity::user_2fa::ActiveModel {
        user_id: sea_orm::Set(1),
        totp_secret: sea_orm::Set("JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP".to_string()),
        algorithm: sea_orm::Set("SHA1".to_string()),
        digits: sea_orm::Set(6),
        period: sea_orm::Set(30),
        enabled: sea_orm::Set(true),
        enabled_at: sea_orm::NotSet,
    };
    user_2fa.insert(server.db.as_ref()).await.unwrap();

    let resp = client
        .login_with_otp("test@example.com", "password123", "000000")
        .await;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    eprintln!("2FA INVALID OTP STATUS: {}, BODY: {}", status, body);
    assert_eq!(status, 400, "body: {}", body);
}

#[tokio::test]
async fn test_ping_success() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;

    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    let resp = client.ping(token).await;
    assert_eq!(resp.status(), 200);

    // Matches seahub's AuthPing, which answers `Response('pong')`.
    let body: String = resp.text().await.unwrap();
    assert_eq!(body, "\"pong\"");
}

#[tokio::test]
async fn test_ping_invalid_token() {
    let server = TestServer::start().await;
    let client = server.client();

    let resp = client
        .ping("invalid_token_40chars____________________")
        .await;
    assert_eq!(resp.status(), 401);
}

// ========== S2FA device trust token tests ==========

fn totp_secret() -> &'static str {
    "JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP"
}

async fn enable_2fa(db: &sea_orm::DatabaseConnection, user_id: i32) {
    let user_2fa = infra::entity::user_2fa::ActiveModel {
        user_id: sea_orm::Set(user_id),
        totp_secret: sea_orm::Set(totp_secret().to_string()),
        algorithm: sea_orm::Set("SHA1".to_string()),
        digits: sea_orm::Set(6),
        period: sea_orm::Set(30),
        enabled: sea_orm::Set(true),
        enabled_at: sea_orm::NotSet,
    };
    user_2fa.insert(db).await.unwrap();
}

fn generate_valid_totp() -> String {
    let totp = server::service::auth::totp::TotpManager::create_totp(
        totp_secret(),
        "test@example.com",
        "",
    )
    .unwrap();
    totp.generate_current().unwrap()
}

#[tokio::test]
async fn test_s2fa_trust_device_flow() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    enable_2fa(server.db.as_ref(), 1).await;

    let valid_code = generate_valid_totp();

    // Step 1: Login with OTP + trust device header
    let resp = client
        .login_with_otp_and_trust_device("test@example.com", "password123", &valid_code)
        .await;
    assert_eq!(resp.status(), 200, "OTP + trust-device should succeed");

    // Read S2FA header BEFORE consuming body (json() takes ownership of resp).
    let s2fa_header = resp
        .headers()
        .get("X-SEAFILE-S2FA")
        .map(|v| v.to_str().unwrap().to_string());
    assert!(
        s2fa_header.is_some(),
        "should return X-SEAFILE-S2FA header when trust-device is set"
    );
    let s2fa_token = s2fa_header.unwrap();
    assert_eq!(s2fa_token.len(), 40);

    // Verify body has API token
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["token"].as_str().unwrap().len() == 40);
    assert!(
        s2fa_token != body["token"].as_str().unwrap(),
        "S2FA token should differ from API token"
    );

    // Step 2: Subsequent login with S2FA token — should skip 2FA
    let resp2 = client
        .login_with_s2fa("test@example.com", "password123", &s2fa_token)
        .await;
    assert_eq!(
        resp2.status(),
        200,
        "S2FA token should bypass 2FA challenge"
    );

    // Step 3: Plain login (no S2FA, no OTP) — should be challenged
    let resp3 = client.login("test@example.com", "password123").await;
    assert_eq!(resp3.status(), 400, "no S2FA/OTP should be challenged");
    assert!(
        resp3.headers().get("X-SEAFILE-OTP").is_some(),
        "should include X-Seafile-OTP: required header"
    );
}

#[tokio::test]
async fn test_s2fa_expired_token() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    enable_2fa(server.db.as_ref(), 1).await;

    // Insert an expired S2FA token directly into the database
    let now = chrono::Utc::now().timestamp();
    let expired_token = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let expired_model = infra::entity::s2fa_token::ActiveModel {
        id: sea_orm::NotSet,
        user_id: sea_orm::Set(1),
        token: sea_orm::Set(server::service::auth::token::hash_token(expired_token)),
        device_id: sea_orm::NotSet,
        device_name: sea_orm::NotSet,
        created_at: sea_orm::Set(now - 100000),
        expires_at: sea_orm::Set(now - 1),
    };
    expired_model.insert(server.db.as_ref()).await.unwrap();

    // Login with expired S2FA → should fall through to OTP challenge
    let resp = client
        .login_with_s2fa("test@example.com", "password123", expired_token)
        .await;
    assert_eq!(resp.status(), 400, "expired S2FA should not bypass 2FA");
    assert!(
        resp.headers().get("X-SEAFILE-OTP").is_some(),
        "should return OTP challenge"
    );

    // Verify the expired token was cleaned up
    let count = infra::entity::s2fa_token::Entity::find()
        .filter(
            infra::entity::s2fa_token::Column::Token
                .eq(server::service::auth::token::hash_token(expired_token)),
        )
        .count(server.db.as_ref())
        .await
        .unwrap();
    assert_eq!(count, 0, "expired S2FA token should have been cleaned up");
}

#[tokio::test]
async fn test_s2fa_invalid_token() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    enable_2fa(server.db.as_ref(), 1).await;

    // Login with a fake S2FA token — should get 2FA challenge
    let fake_token = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let resp = client
        .login_with_s2fa("test@example.com", "password123", fake_token)
        .await;
    assert_eq!(resp.status(), 400, "invalid S2FA should not bypass 2FA");
    assert!(
        resp.headers().get("X-SEAFILE-OTP").is_some(),
        "should return OTP challenge"
    );
}

#[tokio::test]
async fn test_s2fa_no_trust_device() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    enable_2fa(server.db.as_ref(), 1).await;

    let valid_code = generate_valid_totp();

    // Login with OTP but WITHOUT trust device header
    let resp = client
        .login_with_otp("test@example.com", "password123", &valid_code)
        .await;
    assert_eq!(resp.status(), 200, "OTP-only login should succeed");

    // Should NOT have S2FA token in response
    assert!(
        resp.headers().get("X-SEAFILE-S2FA").is_none(),
        "should NOT return S2FA header without trust-device header"
    );
}

#[tokio::test]
async fn test_login_multipart() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;

    // Login with multipart/form-data (matching seadroid's request format)
    let resp = client
        .login_multipart("test@example.com", "password123")
        .await;
    assert_eq!(resp.status(), 200, "multipart login should succeed");

    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();
    assert_eq!(token.len(), 40);
}

#[tokio::test]
async fn test_session_token_stored_hashed() {
    let server = TestServer::start().await;
    let client = server.client();
    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;

    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    let stored = infra::entity::api_token::Entity::find()
        .one(server.db.as_ref())
        .await
        .unwrap()
        .expect("one session token should exist");
    assert_eq!(
        stored.token,
        server::service::auth::token::hash_token(&token),
        "session token must be stored as a SHA-256 hash"
    );
    assert_ne!(stored.token, token);
}

#[tokio::test]
async fn test_sync_token_has_ttl() {
    let server = TestServer::start().await;
    let client = server.client();
    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;

    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    let repo_id = common::create_test_repo(&client, &token, "TTL Test").await;
    let sync = infra::entity::sync_token::Entity::find()
        .filter(infra::entity::sync_token::Column::RepoId.eq(&repo_id))
        .one(server.db.as_ref())
        .await
        .unwrap()
        .expect("sync token should exist");
    let expires_at = sync.expires_at.expect("sync token should have an expiry");
    let now = chrono::Utc::now().timestamp();
    // Test config uses sync_token_ttl_days = 180.
    assert!(
        expires_at > now + 179 * 86400,
        "expiry too soon: {expires_at}"
    );
    assert!(
        expires_at <= now + 181 * 86400,
        "expiry too far: {expires_at}"
    );
}

#[tokio::test]
async fn test_pending_token_rejected_as_session() {
    let server = TestServer::start().await;
    let client = server.client();
    let user_id = create_test_user(server.db.as_ref(), "test@example.com", "password123").await;

    let pending = server::service::auth::token::generate_api_token();
    let now = chrono::Utc::now().timestamp();
    let model = infra::entity::api_token::ActiveModel {
        id: sea_orm::NotSet,
        user_id: sea_orm::Set(user_id),
        token: sea_orm::Set(server::service::auth::token::hash_token(&pending)),
        created_at: sea_orm::Set(now),
        expires_at: sea_orm::Set(Some(now + 300)),
        device_id: sea_orm::Set(None),
        platform: sea_orm::Set(None),
        device_name: sea_orm::Set(None),
        client_version: sea_orm::Set(None),
        is_pending: sea_orm::Set(true),
    };
    model.insert(server.db.as_ref()).await.unwrap();

    let resp = client.ping(&pending).await;
    assert_eq!(
        resp.status(),
        401,
        "pending token must not authenticate as a full session"
    );
}

/// Backup codes verify once, consume on use, and reject wrong codes.
#[tokio::test]
async fn test_backup_code_verify_and_consume() {
    let server = TestServer::start().await;
    let user_id = create_test_user(server.db.as_ref(), "test@example.com", "password123").await;

    let codes = BackupCodeManager::generate_codes(10);
    BackupCodeManager::store_codes(&server.repos, user_id, &codes)
        .await
        .unwrap();

    // A correct code verifies the first time…
    assert!(
        BackupCodeManager::verify_code(&server.repos, user_id, &codes[0])
            .await
            .unwrap()
    );
    // …but is consumed (used) and rejected on a second attempt.
    assert!(
        !BackupCodeManager::verify_code(&server.repos, user_id, &codes[0])
            .await
            .unwrap(),
        "a used backup code must not verify again"
    );
    // Other unused codes still work.
    assert!(
        BackupCodeManager::verify_code(&server.repos, user_id, &codes[1])
            .await
            .unwrap()
    );
    // An unknown code is rejected.
    assert!(
        !BackupCodeManager::verify_code(&server.repos, user_id, &"F".repeat(20))
            .await
            .unwrap()
    );
}
