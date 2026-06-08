mod common;

use common::TestFixture;

/// C.1.2 — POST /seafhttp/repo/{repo_id}/unlock-file?p=path
#[tokio::test]
async fn test_lock_file_success() {
    let f = TestFixture::new().await;

    // Upload a file first
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "lock-me.txt", b"content")
        .await;
    assert!(resp.status().is_success());

    // Lock the file
    let resp = f
        .client
        .post_sync_form(
            &format!("/seafhttp/repo/{}/lock-file?p=/lock-me.txt", f.repo_id),
            &f.sync_token,
            &[],
        )
        .await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_lock_and_unlock_file() {
    let f = TestFixture::new().await;

    // Upload a file
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "file.txt", b"data")
        .await;
    assert!(resp.status().is_success());

    // Lock via sync auth (Seafile-Repo-Token header)
    let resp = f
        .client
        .post_sync_form(
            &format!("/seafhttp/repo/{}/lock-file?p=/file.txt", f.repo_id),
            &f.sync_token,
            &[],
        )
        .await;
    assert_eq!(resp.status(), 200);

    // Unlock
    let resp = f
        .client
        .post_sync_form(
            &format!("/seafhttp/repo/{}/unlock-file?p=/file.txt", f.repo_id),
            &f.sync_token,
            &[],
        )
        .await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_locked_files_list() {
    let f = TestFixture::new().await;

    // Upload and lock a file
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "locked.txt", b"data")
        .await;
    assert!(resp.status().is_success());

    let resp = f
        .client
        .post_sync_form(
            &format!("/seafhttp/repo/{}/lock-file?p=/locked.txt", f.repo_id),
            &f.sync_token,
            &[],
        )
        .await;
    assert_eq!(resp.status(), 200);

    // List locked files (daemon uses POST with JSON body)
    let req_body = serde_json::json!([{
        "repo_id": f.repo_id,
        "token": f.sync_token,
        "ts": 0,
    }]);
    let resp = f
        .client
        .post_sync_json("/seafhttp/repo/locked-files", &f.sync_token, &req_body)
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let list = body.as_array().unwrap();
    assert!(!list.is_empty(), "expected at least one locked file");

    let entry = &list[0];
    assert_eq!(entry["repo_id"], f.repo_id);
    assert_eq!(entry["locked_files"][0]["path"], "/locked.txt");
}

#[tokio::test]
async fn test_locked_files_empty() {
    let f = TestFixture::new().await;

    let req_body = serde_json::json!([{
        "repo_id": f.repo_id,
        "token": f.sync_token,
        "ts": 0,
    }]);
    let resp = f
        .client
        .post_sync_json("/seafhttp/repo/locked-files", &f.sync_token, &req_body)
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert!(arr[0]["locked_files"].as_array().unwrap().is_empty());
}

/// Regression: locked-files must accept POST without Content-Type header.
///
/// seaf-daemon sends POST requests via curl without setting Content-Type.
#[tokio::test]
async fn test_locked_files_accepts_no_content_type() {
    let f = TestFixture::new().await;

    let body = serde_json::json!([{
        "repo_id": f.repo_id,
        "token": f.sync_token,
        "ts": 0,
    }]);

    let resp = f
        .client
        .post_sync_raw("/seafhttp/repo/locked-files", &f.sync_token, &body)
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().unwrap();
    assert_eq!(arr.len(), 1);
}

#[tokio::test]
async fn test_lock_file_requires_auth() {
    let server = common::TestServer::start().await;
    let client = server.client();
    let resp = client
        .post_form("/seafhttp/repo/some-repo/lock-file?p=/f.txt", None, &[])
        .await;
    assert_eq!(resp.status(), 401);
}
