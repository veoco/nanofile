mod common;

use common::{TestServer, create_test_user};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, Set};

#[tokio::test]
async fn test_list_devices_empty() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;

    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    let resp = client.get("/api2/devices/", Some(token)).await;
    assert_eq!(resp.status(), 200);

    let devices: Vec<serde_json::Value> = resp.json().await.unwrap();
    assert!(
        devices.is_empty(),
        "empty user should have no devices: {:?}",
        devices
    );
}

#[tokio::test]
async fn test_list_devices_with_data() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;

    // Login with device info — simulates desktop client
    let resp = client
        .post_form(
            "/api2/auth-token/",
            None,
            &[
                ("username", "test@example.com"),
                ("password", "password123"),
                ("platform", "windows"),
                ("device_id", "dev-001"),
                ("device_name", "my-laptop"),
                ("client_version", "8.0.0"),
            ],
        )
        .await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token1 = body["token"].as_str().unwrap();

    // Login again from a different device
    let resp = client
        .post_form(
            "/api2/auth-token/",
            None,
            &[
                ("username", "test@example.com"),
                ("password", "password123"),
                ("platform", "android"),
                ("device_id", "phone-001"),
                ("device_name", "my-phone"),
                ("client_version", "2.5.0"),
            ],
        )
        .await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token2 = body["token"].as_str().unwrap();

    // List devices — should see both
    let resp = client.get("/api2/devices/", Some(token1)).await;
    assert_eq!(resp.status(), 200);

    let devices: Vec<serde_json::Value> = resp.json().await.unwrap();
    assert_eq!(devices.len(), 2, "should have 2 devices");

    let platforms: Vec<&str> = devices
        .iter()
        .map(|d| d["platform"].as_str().unwrap())
        .collect();
    assert!(platforms.contains(&"windows"), "should include windows");
    assert!(platforms.contains(&"android"), "should include android");

    // Verify desktop client flag
    for device in &devices {
        if device["platform"] == "windows" {
            assert_eq!(device["is_desktop_client"], true);
        }
        if device["platform"] == "android" {
            assert_eq!(device["is_desktop_client"], false);
        }
    }

    // Insert a S2FA token for the android device to verify cleanup.
    let now = chrono::Utc::now().timestamp();
    let s2fa = infra::entity::s2fa_token::ActiveModel {
        id: sea_orm::NotSet,
        user_id: sea_orm::Set(1),
        token: sea_orm::Set(server::service::auth::token::hash_token(
            "cccccccccccccccccccccccccccccccccccccccc",
        )),
        device_id: sea_orm::Set(Some("phone-001".to_string())),
        device_name: sea_orm::Set(Some("my-phone".to_string())),
        created_at: sea_orm::Set(now),
        expires_at: sea_orm::Set(now + 999999),
    };
    s2fa.insert(server.db.as_ref()).await.unwrap();

    // Create a repo via API (this generates a sync token). Then update its
    // peer_id to simulate the sync protocol having stored peer info.
    let repo_id = common::create_test_repo(&client, token1, "Device Test").await;
    let sync_tokens = infra::entity::sync_token::Entity::find()
        .filter(infra::entity::sync_token::Column::RepoId.eq(&repo_id))
        .all(server.db.as_ref())
        .await
        .unwrap();
    assert_eq!(
        sync_tokens.len(),
        1,
        "should have one sync token after repo creation"
    );
    let mut sync_active: infra::entity::sync_token::ActiveModel = sync_tokens[0].clone().into();
    sync_active.peer_id = Set(Some("phone-001".to_string()));
    sync_active.last_sync_time = Set(Some(now));
    sync_active.update(server.db.as_ref()).await.unwrap();

    // Verify tokens exist before unlink
    let count = infra::entity::s2fa_token::Entity::find()
        .count(server.db.as_ref())
        .await
        .unwrap();
    assert_eq!(count, 1, "S2FA token should exist before unlink");
    let count = infra::entity::sync_token::Entity::find()
        .filter(infra::entity::sync_token::Column::PeerId.eq("phone-001"))
        .count(server.db.as_ref())
        .await
        .unwrap();
    assert_eq!(count, 1, "sync token should exist before unlink");

    // Unlink the android device
    let resp = client
        .delete_form(
            "/api2/devices/",
            Some(token1),
            &[("platform", "android"), ("device_id", "phone-001")],
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["success"], true, "unlink should succeed");
    assert!(body["deleted_api_tokens"].as_i64().unwrap() >= 1);
    assert_eq!(
        body["deleted_s2fa_tokens"].as_i64().unwrap(),
        1,
        "S2FA token should be deleted"
    );
    assert_eq!(
        body["deleted_sync_tokens"].as_i64().unwrap(),
        1,
        "sync token should be deleted"
    );

    // Verify tokens were cleaned up
    let count = infra::entity::s2fa_token::Entity::find()
        .count(server.db.as_ref())
        .await
        .unwrap();
    assert_eq!(count, 0, "S2FA token should be deleted after unlink");
    let count = infra::entity::sync_token::Entity::find()
        .filter(infra::entity::sync_token::Column::PeerId.eq("phone-001"))
        .count(server.db.as_ref())
        .await
        .unwrap();
    assert_eq!(count, 0, "sync token should be deleted after unlink");

    // Verify android token no longer works
    let resp = client.ping(token2).await;
    assert_eq!(
        resp.status(),
        401,
        "unlinked device token should be invalid"
    );

    // List devices again — should only see windows
    let resp = client.get("/api2/devices/", Some(token1)).await;
    assert_eq!(resp.status(), 200);
    let devices: Vec<serde_json::Value> = resp.json().await.unwrap();
    assert_eq!(devices.len(), 1, "should only have 1 device after unlink");
    assert_eq!(devices[0]["platform"], "windows");
}

// ============================================================================
// device-wiped (official protocol: anonymous + the device's own API token)
// ============================================================================

/// Compatibility + security: the wipe report is anonymous and carries the
/// device's own API token (the desktop client sends no Authorization header).
#[tokio::test]
async fn test_device_wiped_anonymous_with_token() {
    let server = TestServer::start().await;
    let client = server.client();
    create_test_user(&server.db, "a@example.com", "password123").await;

    let resp = client
        .post_form(
            "/api2/auth-token/",
            None,
            &[
                ("username", "a@example.com"),
                ("password", "password123"),
                ("platform", "linux"),
                ("device_id", "dev-a"),
                ("device_name", "laptop"),
                ("client_version", "9.0.0"),
            ],
        )
        .await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token_a = body["token"].as_str().unwrap().to_string();

    // Anonymous report carrying the device's own token.
    let resp = client
        .post_form("/api2/device-wiped/", None, &[("token", &token_a)])
        .await;
    assert_eq!(resp.status(), 200);

    // The reported token is invalidated.
    let resp = client.ping(&token_a).await;
    assert_eq!(resp.status(), 401, "wiped device token should be invalid");
}

/// Security: a wipe report for one user's device must not delete another
/// user's sessions even if they share the same device_id.
#[tokio::test]
async fn test_device_wiped_scoped_to_user() {
    let server = TestServer::start().await;
    let client = server.client();
    create_test_user(&server.db, "a@example.com", "password123").await;
    create_test_user(&server.db, "b@example.com", "password123").await;

    // Both users login with the SAME device_id.
    for (user, pass) in [
        ("a@example.com", "password123"),
        ("b@example.com", "password123"),
    ] {
        let resp = client
            .post_form(
                "/api2/auth-token/",
                None,
                &[
                    ("username", user),
                    ("password", pass),
                    ("platform", "linux"),
                    ("device_id", "shared-dev"),
                    ("device_name", "x"),
                    ("client_version", "9.0.0"),
                ],
            )
            .await;
        assert_eq!(resp.status(), 200, "login {user} failed");
    }

    // Token for user A.
    let resp = client
        .post_form(
            "/api2/auth-token/",
            None,
            &[
                ("username", "a@example.com"),
                ("password", "password123"),
                ("platform", "linux"),
                ("device_id", "shared-dev"),
                ("device_name", "x"),
                ("client_version", "9.0.0"),
            ],
        )
        .await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token_a = body["token"].as_str().unwrap().to_string();

    // Token for user B.
    let resp = client
        .post_form(
            "/api2/auth-token/",
            None,
            &[
                ("username", "b@example.com"),
                ("password", "password123"),
                ("platform", "linux"),
                ("device_id", "shared-dev"),
                ("device_name", "x"),
                ("client_version", "9.0.0"),
            ],
        )
        .await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token_b = body["token"].as_str().unwrap().to_string();

    // A's token reports a wipe → only A's sessions are removed.
    let resp = client
        .post_form("/api2/device-wiped/", None, &[("token", &token_a)])
        .await;
    assert_eq!(resp.status(), 200);

    let resp = client.ping(&token_a).await;
    assert_eq!(resp.status(), 401, "reported token should be invalid");
    let resp = client.ping(&token_b).await;
    assert_eq!(
        resp.status(),
        200,
        "another user's token must not be deleted"
    );
}
