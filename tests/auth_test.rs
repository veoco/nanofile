mod common;

use common::{TestServer, create_test_user};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};

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
        errors.iter().any(|e| e.as_str().unwrap().contains("Unable to login")),
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
        errors.iter().any(|e| e.as_str().unwrap().contains("Unable to login")),
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
        errors.iter().any(|e| e.as_str().unwrap().contains("Unable to login")),
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
    let user_2fa = nanofile::entity::user_2fa::ActiveModel {
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
    assert_eq!(resp.status(), 401);

    let body: serde_json::Value = resp.json().await.unwrap();
    let err_msg = body["error_msg"].as_str().unwrap();
    assert!(
        err_msg.contains("Two factor auth token is missing"),
        "expected 2FA error message, got: {err_msg}"
    );
}

#[tokio::test]
async fn test_login_2fa_invalid_otp() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;

    let user_2fa = nanofile::entity::user_2fa::ActiveModel {
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
    assert_eq!(status, 401, "body: {}", body);
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

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["email"].as_str().unwrap(), "test@example.com");
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
    let user_2fa = nanofile::entity::user_2fa::ActiveModel {
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
    let totp =
        nanofile::auth::totp::TotpManager::create_totp(totp_secret(), "test@example.com", "")
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
    assert_eq!(resp3.status(), 401, "no S2FA/OTP should be challenged");
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
    let expired_model = nanofile::entity::s2fa_token::ActiveModel {
        id: sea_orm::NotSet,
        user_id: sea_orm::Set(1),
        token: sea_orm::Set(expired_token.to_string()),
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
    assert_eq!(resp.status(), 401, "expired S2FA should not bypass 2FA");
    assert!(
        resp.headers().get("X-SEAFILE-OTP").is_some(),
        "should return OTP challenge"
    );

    // Verify the expired token was cleaned up
    let count = nanofile::entity::s2fa_token::Entity::find()
        .filter(nanofile::entity::s2fa_token::Column::Token.eq(expired_token))
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
    assert_eq!(resp.status(), 401, "invalid S2FA should not bypass 2FA");
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
