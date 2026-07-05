mod common;

use common::{TestFixture, TestServer, create_test_user};
use serde_json::Value;

/// GET /api2/account/info/ returns the display name when set.
#[tokio::test]
async fn test_account_info_returns_display_name() {
    let f = TestFixture::no_repo("test@example.com", "password").await;

    // Set a display name directly in DB
    use sea_orm::{ActiveModelTrait, EntityTrait, Set};
    let user_record = server::entity::user::Entity::find_by_id(f.user_id)
        .one(&*f.server.db)
        .await
        .unwrap()
        .unwrap();
    let mut active: server::entity::user::ActiveModel = user_record.into();
    active.display_name = Set(Some("Alice".to_string()));
    active.update(&*f.server.db).await.unwrap();

    // GET /api2/account/info/
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

/// GET /api2/account/info/ falls back to the local part of the email when
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
    // No display_name or name set -> falls back to local part
    assert_eq!(body["name"], "john.doe");
    assert_eq!(body["nickname"], "john.doe");
}

/// GET /api2/account/info/ uses the name column when display_name is not set.
#[tokio::test]
async fn test_account_info_uses_name_when_display_name_unset() {
    let server = TestServer::start().await;
    let client = server.client();

    use sea_orm::{ActiveModelTrait, EntityTrait, Set};
    use server::entity::user;

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

/// PUT /api2/account/info/ updates the display name.
#[tokio::test]
async fn test_put_account_info_updates_display_name() {
    let f = TestFixture::no_repo("test@example.com", "password").await;

    // PUT with new display name
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

    // Verify it persists via GET
    let resp = f
        .client
        .get("/api2/account/info/", Some(&f.api_token))
        .await;
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "My Display Name");
}

/// PUT /api2/account/info/ with an empty name clears the display name,
/// falling back to the email local part.
#[tokio::test]
async fn test_put_account_info_empty_name_clears_display_name() {
    let f = TestFixture::no_repo("test@example.com", "password").await;

    // First set a display name
    f.client
        .put_json(
            "/api2/account/info/",
            Some(&f.api_token),
            &serde_json::json!({"name": "Temporary"}),
        )
        .await;

    // Then clear it with empty string
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
    // Falls back to local part of email
    assert_eq!(body["name"], "test");
}

/// GET /api2/account/info/ returns 401 without auth.
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

/// GET /api2/account/info/ returns total = -1 when storage_quota = 0 (unlimited).
#[tokio::test]
async fn test_account_info_quota_unlimited() {
    let f = TestFixture::no_repo("test@example.com", "password").await;

    // Set storage_quota to 0 (explicitly unlimited)
    use sea_orm::{ActiveModelTrait, EntityTrait, Set};
    let user_record = server::entity::user::Entity::find_by_id(f.user_id)
        .one(&*f.server.db)
        .await
        .unwrap()
        .unwrap();
    let mut active: server::entity::user::ActiveModel = user_record.into();
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

/// GET /api2/account/info/ returns the user's storage_quota when set to a specific value.
#[tokio::test]
async fn test_account_info_quota_user_specific() {
    let f = TestFixture::no_repo("test@example.com", "password").await;

    // Set storage_quota to 1 GB
    use sea_orm::{ActiveModelTrait, EntityTrait, Set};
    let user_record = server::entity::user::Entity::find_by_id(f.user_id)
        .one(&*f.server.db)
        .await
        .unwrap()
        .unwrap();
    let mut active: server::entity::user::ActiveModel = user_record.into();
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

/// GET /api2/account/info/ returns global quota when storage_quota is None.
#[tokio::test]
async fn test_account_info_quota_fallback_global() {
    let f = TestFixture::no_repo("test@example.com", "password").await;

    // storage_quota is None by default -> should return global max_storage_bytes (10 GB)
    let resp = f
        .client
        .get("/api2/account/info/", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["total"], 10_737_418_240_i64); // 10 GB from test config
}
