mod common;

use common::TestFixture;

/// C.2.1 — GET /seafhttp/repo/{repo_id}/jwt-token
#[tokio::test]
async fn test_jwt_token_success() {
    let f = TestFixture::new_with_notification().await;

    let resp = f
        .client
        .get_sync(
            &format!("/seafhttp/repo/{}/jwt-token", f.repo_id),
            &f.sync_token,
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body["jwt_token"].as_str().unwrap_or("").len() > 20,
        "expected JWT token, got: {:?}",
        body
    );
}

#[tokio::test]
async fn test_jwt_token_unauthorized() {
    let server = common::TestServer::start().await;
    let client = server.client();
    let resp = client.get("/seafhttp/repo/some-repo/jwt-token", None).await;
    assert_eq!(resp.status(), 401);
}

/// C.2.2 — POST /seafhttp/repo/folder-perm
#[tokio::test]
async fn test_folder_perm_returns_list() {
    let f = TestFixture::new().await;

    let body = serde_json::json!([{
        "repo_id": f.repo_id,
        "token": f.sync_token,
        "ts": 0,
    }]);

    let resp = f
        .client
        .post_sync_json("/seafhttp/repo/folder-perm", &f.sync_token, &body)
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().unwrap();
    assert!(!arr.is_empty());
    assert_eq!(arr[0]["repo_id"].as_str().unwrap(), f.repo_id);
    assert!(arr[0]["user_perms"].as_array().unwrap().len() >= 1);
}

#[tokio::test]
async fn test_folder_perm_unauthorized() {
    let server = common::TestServer::start().await;
    let client = server.client();
    // POST with invalid token — any repo_id/token pair that doesn't exist
    // returns empty user_perms for that entry (not an error).
    let body = serde_json::json!([{
        "repo_id": "00000000-0000-0000-0000-000000000000",
        "token": "invalid_token_that_does_not_exist",
        "ts": 0,
    }]);
    let resp = client
        .post_sync_json("/seafhttp/repo/folder-perm", "ignored", &body)
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert!(arr[0]["user_perms"].as_array().unwrap().is_empty());
    assert!(arr[0]["group_perms"].as_array().unwrap().is_empty());
}

/// Regression: folder-perm must accept POST without Content-Type header.
///
/// seaf-daemon sends POST requests via curl without setting Content-Type.
/// Before the fix, axum's Json extractor would reject these with 415.
#[tokio::test]
async fn test_folder_perm_accepts_no_content_type() {
    let f = TestFixture::new().await;

    let body = serde_json::json!([{
        "repo_id": f.repo_id,
        "token": f.sync_token,
        "ts": 0,
    }]);

    let resp = f
        .client
        .post_sync_raw("/seafhttp/repo/folder-perm", &f.sync_token, &body)
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().unwrap();
    assert!(!arr.is_empty());
    assert_eq!(arr[0]["repo_id"].as_str().unwrap(), f.repo_id);
    assert!(arr[0]["user_perms"].as_array().unwrap().len() >= 1);
}

/// C.2.3 — GET /seafhttp/accessible-repos
#[tokio::test]
async fn test_accessible_repos_returns_list() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .get_sync("/seafhttp/accessible-repos", &f.sync_token)
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let repos = body.as_array().unwrap();
    assert!(!repos.is_empty(), "should have at least one repo");
    assert!(repos[0]["repo_id"].as_str().unwrap_or("").len() > 0);
    assert!(repos[0]["token"].as_str().unwrap_or("").len() > 0);
}

#[tokio::test]
async fn test_accessible_repos_unauthorized() {
    let server = common::TestServer::start().await;
    let client = server.client();
    let resp = client.get("/seafhttp/accessible-repos", None).await;
    assert_eq!(resp.status(), 401);
}
