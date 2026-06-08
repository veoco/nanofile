mod common;

use common::{TestServer, create_test_user};

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
