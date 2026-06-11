mod common;

use common::{TestFixture, TestServer, create_test_user};

#[tokio::test]
async fn test_create_repo() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    let resp = client.create_repo(token, "My Library").await;
    assert_eq!(resp.status(), 201);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["id"].as_str().is_some());
    assert_eq!(body["name"].as_str().unwrap(), "My Library");
}

#[tokio::test]
async fn test_list_repos() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    client.create_repo(token, "Lib1").await;
    client.create_repo(token, "Lib2").await;

    let resp = client.list_repos(token).await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let repos = body.as_array().unwrap();
    assert_eq!(repos.len(), 2);
}

#[tokio::test]
async fn test_get_repo() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    let resp = client.create_repo(token, "My Library").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let repo_id = body["id"].as_str().unwrap();

    let resp = client.get_repo(token, repo_id).await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"].as_str().unwrap(), "My Library");
}

#[tokio::test]
async fn test_download_info() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, token, "My Library").await;

    let resp = client.download_info(token, &repo_id).await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["token"].as_str().is_some());
    assert_eq!(body["repo_id"].as_str().unwrap(), repo_id);
}

/// Regression: download-info must return all fields required by seaf-cli
/// (email, repo_name, repo_version, salt, permission, encrypted, magic, random_key, enc_version)
#[tokio::test]
async fn test_download_info_fields_complete() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, token, "My Library").await;

    let resp = client.download_info(token, &repo_id).await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();

    // Required by seaf-cli (KeyError if missing)
    assert_eq!(body["email"].as_str().unwrap(), "test@example.com");
    assert_eq!(body["repo_name"].as_str().unwrap(), "My Library");
    assert_eq!(body["repo_id"].as_str().unwrap(), repo_id);
    assert!(body["token"].as_str().unwrap().len() >= 40);

    // Required by seaf-daemon clone (uses .get() with defaults, but must be present)
    assert_eq!(body["repo_version"].as_i64().unwrap(), 1);
    // salt: None → null in JSON
    assert!(body["salt"].is_null());
    assert_eq!(body["permission"].as_str().unwrap(), "rw");

    // Encryption-related fields
    assert_eq!(body["encrypted"].as_str().unwrap(), "false");
    assert_eq!(body["enc_version"].as_i64().unwrap(), 0);
    assert!(body["magic"].is_null());
    assert!(body["random_key"].is_null());

    // Relay fields (not used but should be present)
    assert!(body["relay_id"].is_null());
    assert!(body["relay_addr"].is_null());
    assert!(body["relay_port"].is_null());
}

/// Regression: trailing slashes on API routes
#[tokio::test]
async fn test_trailing_slash_routes() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    // All these should return 200 (not 404), matching Seafile client URLs
    let repo_id = common::create_test_repo(&client, token, "Trailing Test").await;

    // GET /api2/repos/ with trailing slash
    let resp = client.list_repos(token).await;
    assert_eq!(resp.status(), 200);

    // GET /api2/repos/{id}/ with trailing slash
    let resp = client.get_repo(token, &repo_id).await;
    assert_eq!(resp.status(), 200);

    // GET /api2/repos/{id}/download-info/ with trailing slash
    let resp = client.download_info(token, &repo_id).await;
    assert_eq!(resp.status(), 200);
}

/// Regression: create repo with trailing slash
#[tokio::test]
async fn test_create_repo_trailing_slash() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "ci@test.com", "ci123456").await;
    let resp = client.login("ci@test.com", "ci123456").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    // POST /api2/repos/ (with trailing slash) must return 201
    let resp = client.create_repo(token, "Test Repo").await;
    assert_eq!(resp.status(), 201);
}

#[tokio::test]
async fn test_delete_repo() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, token, "My Library").await;

    let resp = client.delete_repo(token, &repo_id).await;
    assert_eq!(resp.status(), 200);

    let resp = client.get_repo(token, &repo_id).await;
    assert_eq!(resp.status(), 404);
}

/// B.11.1 — POST /api2/repos/{repo_id}/?op=rename — rename repo.
#[tokio::test]
async fn test_rename_repo_success() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .rename_repo(&f.api_token, &f.repo_id, "NewName")
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["success"], true);

    // Verify via GET.
    let resp = f.client.get_repo(&f.api_token, &f.repo_id).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "NewName");
}

/// Regression: rename repo via multipart POST (Android client format).
#[tokio::test]
async fn test_rename_repo_multipart_body() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .rename_repo_multipart(&f.api_token, &f.repo_id, "MultipartName")
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["success"], true);

    // Verify via GET.
    let resp = f.client.get_repo(&f.api_token, &f.repo_id).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "MultipartName");
}

/// B.11.2 — Non-owner cannot rename a repo.
#[tokio::test]
async fn test_rename_repo_non_owner() {
    let f = TestFixture::new().await;

    // Create a second user.
    create_test_user(f.server.db.as_ref(), "other@test.com", "password").await;
    let resp = f.client.login("other@test.com", "password").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let other_token = body["token"].as_str().unwrap();

    let resp = f
        .client
        .rename_repo(other_token, &f.repo_id, "Hacked")
        .await;
    assert_eq!(resp.status(), 403);
}

/// B.11.3 — Invalid name returns 400.
#[tokio::test]
async fn test_rename_repo_invalid_name() {
    let f = TestFixture::new().await;

    // Empty name.
    let resp = f.client.rename_repo(&f.api_token, &f.repo_id, "").await;
    assert_eq!(resp.status(), 400);

    // Name with slash.
    let resp = f
        .client
        .rename_repo(&f.api_token, &f.repo_id, "bad/name")
        .await;
    assert_eq!(resp.status(), 400);
}

/// B.11.4 — Non-owner cannot delete a repo.
#[tokio::test]
async fn test_delete_repo_non_owner() {
    let f = TestFixture::new().await;

    // Create a second user.
    create_test_user(f.server.db.as_ref(), "other2@test.com", "password").await;
    let resp = f.client.login("other2@test.com", "password").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let other_token = body["token"].as_str().unwrap();

    let resp = f.client.delete_repo(other_token, &f.repo_id).await;
    assert_eq!(resp.status(), 403);
}

/// B.11.5 — DELETE /api/v2.1/repos/{repo_id}/ — delete repo via v2.1 API.
#[tokio::test]
async fn test_delete_repo_v21_success() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .delete(
            &format!("/api/v2.1/repos/{}/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);

    // Verify it's gone.
    let resp = f.client.get_repo(&f.api_token, &f.repo_id).await;
    assert_eq!(resp.status(), 404);
}

/// B.11.6 — Non-owner cannot delete a repo via v2.1.
#[tokio::test]
async fn test_delete_repo_v21_non_owner() {
    let f = TestFixture::new().await;

    // Create a second user.
    create_test_user(f.server.db.as_ref(), "other3@test.com", "password").await;
    let resp = f.client.login("other3@test.com", "password").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let other_token = body["token"].as_str().unwrap();

    let resp = f
        .client
        .delete(
            &format!("/api/v2.1/repos/{}/", f.repo_id),
            Some(other_token),
        )
        .await;
    assert_eq!(resp.status(), 403);
}

/// B.11.7 — POST /api2/repos/ accepts JSON body (web frontend format).
#[tokio::test]
async fn test_create_repo_json_body() {
    let f = TestFixture::new().await;

    // Create a repo with JSON body (as the web frontend does).
    let resp = f
        .client
        .post_json(
            "/api2/repos/",
            Some(&f.api_token),
            &serde_json::json!({"name": "JSON Created Repo"}),
        )
        .await;
    assert_eq!(resp.status(), 201);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "JSON Created Repo");
    assert!(body["id"].as_str().is_some());
}
