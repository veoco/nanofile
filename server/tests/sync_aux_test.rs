use crate::common;

use common::TestFixture;

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
    // Missing credentials → 400 (seafile fileserver returns BADREQ), which
    // seaf-daemon classifies as a general request error.
    assert_eq!(resp.status(), 400);
}

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
    assert!(!arr[0]["user_perms"].as_array().unwrap().is_empty());
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
    assert!(!arr[0]["user_perms"].as_array().unwrap().is_empty());
}

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
    assert_eq!(repos.len(), 1, "should have exactly the fixture library");

    // SeaDrive parses this response with `parse_repo_list()` (seadrive
    // `sync-mgr.c`), which requires seafile-server's `share.SharedRepo` members:
    // `version`, `id`, `head_commit_id`, `name`, `mtime`, `permission`, `type`,
    // `owner`. A renamed or missing member fails the whole list.
    let repo = &repos[0];
    assert_eq!(repo["version"].as_i64().unwrap(), 1);
    assert_eq!(repo["id"].as_str().unwrap(), f.repo_id.as_str());
    assert_eq!(
        repo["head_commit_id"].as_str().unwrap(),
        "0000000000000000000000000000000000000000",
        "a headless library reports the empty-head sentinel"
    );
    assert_eq!(repo["name"].as_str().unwrap(), "test-repo");
    assert!(repo["mtime"].as_i64().unwrap() > 0);
    assert_eq!(repo["permission"].as_str().unwrap(), "rw");
    assert_eq!(repo["type"].as_str().unwrap(), "repo");
    assert_eq!(repo["owner"].as_str().unwrap(), "test@example.com");
    assert!(
        repo.get("token").is_none(),
        "the fileserver response carries no per-library token"
    );
}

#[tokio::test]
async fn test_accessible_repos_unauthorized() {
    let server = common::TestServer::start().await;
    let client = server.client();
    let resp = client.get("/seafhttp/accessible-repos", None).await;
    // Missing credentials → 400 (see test_jwt_token_unauthorized).
    assert_eq!(resp.status(), 400);
}

/// Security hardening: head-commits-multi must reject malformed repo ids
/// (this endpoint is intentionally unauthenticated for seaf-daemon, matching
/// official seafile, so input validation is the mitigation).
#[tokio::test]
async fn test_head_commits_multi_rejects_malformed_ids() {
    let f = TestFixture::new().await;
    let resp = f.client.head_commits_multi(&["not-a-uuid"]).await;
    assert_eq!(resp.status(), 400);
}

/// Security hardening: head-commits-multi must reject oversized request arrays.
#[tokio::test]
async fn test_head_commits_multi_rejects_oversized_array() {
    let f = TestFixture::new().await;
    let too_many: Vec<String> = (0..5000)
        .map(|i| format!("{i:0>8}-0000-4000-8000-000000000000"))
        .collect();
    let refs: Vec<&str> = too_many.iter().map(|s| s.as_str()).collect();
    let resp = f.client.head_commits_multi(&refs).await;
    assert_eq!(resp.status(), 400);
}
