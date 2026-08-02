mod common;

use common::TestFixture;

/// B.10.6 — GET /api2/unseen_messages/
#[tokio::test]
async fn test_unseen_messages_returns_count() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .get("/api2/unseen_messages/", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["count"], 0);
}

#[tokio::test]
async fn test_unseen_messages_unauthorized() {
    let server = common::TestServer::start().await;
    let client = server.client();
    let resp = client.get("/api2/unseen_messages/", None).await;
    assert_eq!(resp.status(), 401);
}

/// B.10.8 — POST /api2/logout-device/
#[tokio::test]
async fn test_logout_device_success() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .post_form("/api2/logout-device/", Some(&f.api_token), &[])
        .await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_logout_device_token_invalidated() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .post_form("/api2/logout-device/", Some(&f.api_token), &[])
        .await;
    assert_eq!(resp.status(), 200);

    let resp = f.client.get("/api2/auth/ping/", Some(&f.api_token)).await;
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_logout_device_unauthorized() {
    let server = common::TestServer::start().await;
    let client = server.client();
    let resp = client.post_form("/api2/logout-device/", None, &[]).await;
    assert_eq!(resp.status(), 401);
}

/// B.10.5 — GET /api2/repo-tokens/?repos=id1,id2
#[tokio::test]
async fn test_repo_tokens_single_repo() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .get(
            &format!("/api2/repo-tokens/?repos={}", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        !body[&f.repo_id].as_str().unwrap().is_empty(),
        "should have a token"
    );
}

#[tokio::test]
async fn test_repo_tokens_multiple_repos() {
    let f = TestFixture::new().await;

    let repo2_id = common::create_test_repo(&f.client, &f.api_token, "second-repo").await;

    let resp = f
        .client
        .get(
            &format!("/api2/repo-tokens/?repos={},{}", f.repo_id, repo2_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(!body[&f.repo_id].as_str().unwrap().is_empty());
    assert!(!body[&repo2_id].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn test_repo_tokens_unauthorized() {
    let server = common::TestServer::start().await;
    let client = server.client();
    let resp = client.get("/api2/repo-tokens/?repos=abc", None).await;
    assert_eq!(resp.status(), 401);
}

/// B.10.2 — GET /api2/repos/{repo_id}/dir/shared_items/
#[tokio::test]
async fn test_dir_shared_items_returns_list() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/dir/shared_items/?p=/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["shared_items"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_dir_shared_items_unauthorized() {
    let server = common::TestServer::start().await;
    let client = server.client();
    let resp = client
        .get("/api2/repos/some-repo/dir/shared_items/?p=/", None)
        .await;
    assert_eq!(resp.status(), 401);
}

/// B.10.7 — POST /api2/beshared-repos/{repo_id}/
#[tokio::test]
async fn test_beshared_repos_no_user_returns_400() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .post_json(
            &format!("/api2/beshared-repos/{}/", f.repo_id),
            Some(&f.api_token),
            &serde_json::json!({"share_type": "personal", "user": ""}),
        )
        .await;
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn test_beshared_repos_unauthorized() {
    let server = common::TestServer::start().await;
    let client = server.client();
    let resp = client
        .post_json(
            "/api2/beshared-repos/some-repo/",
            None,
            &serde_json::json!({"share_type": "personal", "user": "test@test.com"}),
        )
        .await;
    assert_eq!(resp.status(), 401);
}

/// B.10.3 — GET /api2/repo_history_changes/{repo_id}/?commit_id=
#[tokio::test]
async fn test_repo_history_changes_no_commits() {
    let f = TestFixture::new().await;

    // Upload a file to create a commit
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "history.txt", b"test")
        .await;
    assert!(
        resp.status().is_success(),
        "upload failed: {}",
        resp.status()
    );

    // Get the head commit
    let resp = f
        .client
        .get_sync(
            &format!("/seafhttp/repo/{}/commit/HEAD", f.repo_id),
            &f.sync_token,
        )
        .await;
    assert_eq!(
        resp.status(),
        200,
        "failed to get HEAD commit: {}",
        resp.status()
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body["head_commit_id"].is_string(),
        "no head_commit_id in HEAD response: {:?}",
        body
    );
    let commit_id = body["head_commit_id"].as_str().unwrap();

    // Get history changes for this commit
    let resp = f
        .client
        .get(
            &format!(
                "/api2/repo_history_changes/{}/?commit_id={}",
                f.repo_id, commit_id
            ),
            Some(&f.api_token),
        )
        .await;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    assert_eq!(
        status, 200,
        "repo_history_changes failed ({}): {}",
        status, text
    );
    let body: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    // Should have at least the uploaded file
    assert!(
        body["new_files"].as_array().is_some() || body["modified_files"].as_array().is_some(),
        "expected new_files or modified_files in: {:?}",
        body
    );
}

#[tokio::test]
async fn test_repo_history_changes_invalid_commit() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .get(
            &format!(
                "/api2/repo_history_changes/{}/?commit_id=0000000000000000000000000000000000000000",
                f.repo_id
            ),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn test_repo_history_changes_unauthorized() {
    let server = common::TestServer::start().await;
    let client = server.client();
    let resp = client.get(
        "/api2/repo_history_changes/some-repo/?commit_id=0000000000000000000000000000000000000000",
        None,
    ).await;
    assert_eq!(resp.status(), 401);
}

/// B.10.1 — GET /api2/repos/{repo_id}/dir/sub_repo/
#[tokio::test]
async fn test_sub_repo_create() {
    let f = TestFixture::new().await;

    // Create a directory in the repo
    let resp = f
        .client
        .create_dir(&f.api_token, &f.repo_id, "/subdir")
        .await;
    assert!(resp.status().is_success());

    // Create sub-repo from this directory
    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/dir/sub_repo/?p=/subdir", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(status, 200, "sub_repo failed: {:?}", body);
    assert!(
        !body["id"].as_str().unwrap_or("").is_empty(),
        "no repo id: {:?}",
        body
    );
    assert!(
        body["name"].as_str().unwrap_or("").contains("subdir"),
        "expected name with subdir"
    );
}

#[tokio::test]
async fn test_sub_repo_invalid_path() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/dir/sub_repo/?p=/nonexistent", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn test_sub_repo_unauthorized() {
    let server = common::TestServer::start().await;
    let client = server.client();
    let resp = client
        .get(
            &format!("/api2/repos/{}/dir/sub_repo/?p=/subdir", "some-repo"),
            None,
        )
        .await;
    assert_eq!(resp.status(), 401);
}

// ── File history / revision / restore (web UI 文件历史版本) ──────────────

/// 上传（或覆盖）`/hist.txt`，每次调用产生一个内容版本。
async fn upload_hist_version(f: &TestFixture, data: &[u8]) {
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "hist.txt", data)
        .await;
    assert!(
        resp.status().is_success(),
        "upload failed: {}",
        resp.status()
    );
}

#[tokio::test]
async fn test_file_history_v21_unauthorized() {
    let server = common::TestServer::start().await;
    let client = server.client();
    let resp = client
        .get("/api/v2.1/repos/some-repo/file/history/?p=/a.txt", None)
        .await;
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_file_history_v21_no_such_file() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .get(
            &format!("/api/v2.1/repos/{}/file/history/?p=/nope.txt", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["data"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_file_history_v21_lists_versions() {
    let f = TestFixture::new().await;
    for data in [b"version-1".as_slice(), b"version-2", b"version-3"] {
        upload_hist_version(&f, data).await;
    }

    let resp = f
        .client
        .get(
            &format!("/api/v2.1/repos/{}/file/history/?p=/hist.txt", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 3, "expected 3 versions, got: {:?}", data);

    // 最新版本在列表最前
    let newest = &data[0];
    assert!(newest["commit_id"].is_string());
    assert_eq!(newest["path"], "/hist.txt");
    assert_eq!(newest["size"].as_i64(), Some(b"version-3".len() as i64));
    assert!(newest["mtime"].is_number());
    assert!(newest["last_modified_by"].is_string());
    assert!(newest["rev_file_id"].is_string());
    assert!(newest["rev_file_name"].is_string());

    // 最旧版本是 v1
    assert_eq!(data[2]["size"].as_i64(), Some(b"version-1".len() as i64));
}

#[tokio::test]
async fn test_file_revision_v21_downloads_old_version() {
    let f = TestFixture::new().await;
    upload_hist_version(&f, b"first-content").await;
    upload_hist_version(&f, b"second-content").await;

    let resp = f
        .client
        .get(
            &format!("/api/v2.1/repos/{}/file/history/?p=/hist.txt", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 2);
    let old_commit = data[1]["commit_id"].as_str().unwrap().to_string();

    let resp = f
        .client
        .get(
            &format!(
                "/api/v2.1/repos/{}/file/revision/?p=/hist.txt&commit_id={}",
                f.repo_id, old_commit
            ),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let bytes = resp.bytes().await.unwrap();
    assert_eq!(bytes.as_ref(), b"first-content");
}

#[tokio::test]
async fn test_file_revision_v21_invalid_commit() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .get(
            &format!(
                "/api/v2.1/repos/{}/file/revision/?p=/hist.txt&commit_id=0000000000000000000000000000000000000000",
                f.repo_id
            ),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn test_restore_file_revision_v21() {
    let f = TestFixture::new().await;
    for data in [b"version-1".as_slice(), b"version-2", b"version-3"] {
        upload_hist_version(&f, data).await;
    }

    let resp = f
        .client
        .get(
            &format!("/api/v2.1/repos/{}/file/history/?p=/hist.txt", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let data = body["data"].as_array().unwrap();
    let old_commit = data[2]["commit_id"].as_str().unwrap().to_string();

    let resp = f
        .client
        .post_json(
            &format!(
                "/api/v2.1/repos/{}/file/revision/restore/?p=/hist.txt&commit_id={}",
                f.repo_id, old_commit
            ),
            Some(&f.api_token),
            &serde_json::json!({}),
        )
        .await;
    assert_eq!(
        resp.status(),
        200,
        "restore failed: {}",
        resp.text().await.unwrap_or_default()
    );

    // 恢复后当前文件内容应为 v1
    let resp = f
        .client
        .download_file(&f.api_token, &f.repo_id, "/hist.txt")
        .await;
    assert!(resp.status().is_success());
    let bytes = resp.bytes().await.unwrap();
    assert_eq!(bytes.as_ref(), b"version-1");
}

#[tokio::test]
async fn test_restore_file_revision_v21_requires_write() {
    let server = common::TestServer::start().await;
    let client = server.client();
    let db = &*server.db;

    let _owner_id = common::create_test_user(db, "owner@example.com", "password").await;
    let _other_id = common::create_test_user(db, "other@example.com", "password").await;

    let resp = client.login("owner@example.com", "password").await;
    assert_eq!(resp.status(), 200, "owner login failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    let owner_token = body["token"].as_str().unwrap().to_string();

    let repo_id = common::create_test_repo(&client, &owner_token, "repo").await;
    let resp = client
        .upload_file(&owner_token, &repo_id, "/", "hist.txt", b"version-1")
        .await;
    assert!(resp.status().is_success());

    let resp = client.login("other@example.com", "password").await;
    assert_eq!(resp.status(), 200, "other login failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    let other_token = body["token"].as_str().unwrap().to_string();

    let resp = client
        .post_json(
            &format!(
                "/api/v2.1/repos/{}/file/revision/restore/?p=/hist.txt&commit_id=0000000000000000000000000000000000000000",
                repo_id
            ),
            Some(&other_token),
            &serde_json::json!({}),
        )
        .await;
    assert_eq!(resp.status(), 403);
}
