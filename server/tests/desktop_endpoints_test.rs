use crate::common;

use common::TestFixture;
use sea_orm::EntityTrait;

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
///
/// Administrator-managed library sharing: a normal-account owner is outside the
/// policy and gets 403. The administrator shape (bare array of user shares) is
/// covered by `library_sharing_test`.
#[tokio::test]
async fn test_dir_shared_items_requires_an_administrator() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/dir/shared_items/?p=/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 403);
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

/// B.10.3 — GET /api2/repo_history_changes/{repo_id}/?commit_id=
#[tokio::test]
async fn test_repo_history_changes_no_commits() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "history.txt", b"test")
        .await;
    assert!(
        resp.status().is_success(),
        "upload failed: {}",
        resp.status()
    );

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

    let resp = f
        .client
        .create_dir(&f.api_token, &f.repo_id, "/subdir")
        .await;
    assert!(resp.status().is_success());

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

/// A library created from a folder must be able to serve the folder's files.
/// Blocks are stored per library, so `copy_fs_tree` copying only the FS objects
/// is not enough: the referenced blocks have to be copied into the new
/// library's block directory, otherwise every download from it 404s (or the
/// file reports missing blocks).
#[tokio::test]
async fn test_sub_repo_copies_referenced_blocks() {
    let f = TestFixture::new().await;
    let content = b"content that must follow the folder into the new library".to_vec();

    assert!(
        f.client
            .create_dir(&f.api_token, &f.repo_id, "/subdir")
            .await
            .status()
            .is_success()
    );
    assert!(
        f.client
            .upload_file(&f.api_token, &f.repo_id, "/subdir", "inside.txt", &content)
            .await
            .status()
            .is_success()
    );

    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/dir/sub_repo/?p=/subdir", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let new_repo = body["id"].as_str().unwrap().to_string();

    let created = infra::entity::repo::Entity::find_by_id(new_repo.clone())
        .one(&*f.server.db)
        .await
        .unwrap()
        .expect("new library row");
    assert_eq!(
        created.size,
        content.len() as i64,
        "the sub-library must be charged for the copied subtree"
    );

    let resp = f
        .client
        .download_file(&f.api_token, &new_repo, "/inside.txt")
        .await;
    assert_eq!(resp.status(), 200, "download from the new library failed");
    assert_eq!(resp.bytes().await.unwrap().to_vec(), content);
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

    let newest = &data[0];
    assert!(newest["commit_id"].is_string());
    assert_eq!(newest["path"], "/hist.txt");
    assert_eq!(newest["size"].as_i64(), Some(b"version-3".len() as i64));
    assert!(newest["mtime"].is_number());
    assert!(newest["last_modified_by"].is_string());
    assert!(newest["rev_file_id"].is_string());
    assert!(newest["rev_file_name"].is_string());

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
                "/api2/repos/{}/file/revision/?p=/hist.txt&commit_id={}",
                f.repo_id, old_commit
            ),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let bytes = resp.bytes().await.unwrap();
    assert_eq!(bytes.as_ref(), b"first-content");
}

/// A revision is served by the shared response builder: Range, ETag and 304.
#[tokio::test]
async fn test_file_revision_v21_supports_ranges_and_validators() {
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
    let commit = body["data"][1]["commit_id"].as_str().unwrap().to_string();

    let url = format!(
        "{}/api2/repos/{}/file/revision/?p=/hist.txt&commit_id={commit}",
        f.server.base_url, f.repo_id
    );
    let http = reqwest::Client::builder().no_proxy().build().unwrap();

    let resp = http
        .get(&url)
        .bearer_auth(&f.api_token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok()),
        Some("13"),
        "revision responses must advertise their length"
    );
    let etag = resp
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .expect("revision ETag missing")
        .to_string();

    let resp = http
        .get(&url)
        .bearer_auth(&f.api_token)
        .header("range", "bytes=0-4")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 206);
    assert_eq!(
        resp.headers()
            .get("content-range")
            .and_then(|v| v.to_str().ok()),
        Some("bytes 0-4/13")
    );
    assert_eq!(resp.bytes().await.unwrap().as_ref(), b"first");

    let resp = http
        .get(&url)
        .bearer_auth(&f.api_token)
        .header("if-none-match", &etag)
        .header("range", "bytes=0-4")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 304);
}

#[tokio::test]
async fn test_file_revision_v21_invalid_commit() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .get(
            &format!(
                "/api2/repos/{}/file/revision/?p=/hist.txt&commit_id=0000000000000000000000000000000000000000",
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
        .post_form(
            &format!("/api2/repos/{}/file/?p=/hist.txt", f.repo_id),
            Some(&f.api_token),
            &[("operation", "revert"), ("commit_id", old_commit.as_str())],
        )
        .await;
    assert_eq!(
        resp.status(),
        200,
        "restore failed: {}",
        resp.text().await.unwrap_or_default()
    );

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
        .post_form(
            &format!("/api2/repos/{}/file/?p=/hist.txt", repo_id),
            Some(&other_token),
            &[
                ("operation", "revert"),
                ("commit_id", "0000000000000000000000000000000000000000"),
            ],
        )
        .await;
    assert_eq!(resp.status(), 403);
}

#[tokio::test]
async fn test_revert_recreates_a_deleted_file() {
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
    let old_commit = body["data"][1]["commit_id"].as_str().unwrap().to_string();

    let resp = f
        .client
        .delete(
            &format!("/api2/repos/{}/file/?p=/hist.txt", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert!(resp.status().is_success(), "delete failed");

    let resp = f
        .client
        .post_form(
            &format!("/api2/repos/{}/file/?p=/hist.txt", f.repo_id),
            Some(&f.api_token),
            &[("operation", "revert"), ("commit_id", old_commit.as_str())],
        )
        .await;
    assert_eq!(
        resp.status(),
        200,
        "reverting a deleted path must recreate it: {}",
        resp.text().await.unwrap_or_default()
    );

    let resp = f
        .client
        .download_file(&f.api_token, &f.repo_id, "/hist.txt")
        .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.bytes().await.unwrap().as_ref(), b"first-content");
}

#[tokio::test]
async fn test_revert_rejects_a_file_locked_by_another_user() {
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
    let old_commit = body["data"][1]["commit_id"].as_str().unwrap().to_string();

    let other_id = common::create_test_user(&f.server.db, "locker@example.com", "password").await;
    f.server
        .repos
        .locked_file
        .create(
            &f.repo_id,
            "hist.txt",
            other_id,
            chrono::Utc::now().timestamp(),
            "locker@example.com",
        )
        .await
        .unwrap();

    let resp = f
        .client
        .post_form(
            &format!("/api2/repos/{}/file/?p=/hist.txt", f.repo_id),
            Some(&f.api_token),
            &[("operation", "revert"), ("commit_id", old_commit.as_str())],
        )
        .await;
    assert_eq!(
        resp.status(),
        403,
        "a foreign lock must reject the revert: {}",
        resp.text().await.unwrap_or_default()
    );

    f.server
        .repos
        .locked_file
        .delete_by_repo_and_path(&f.repo_id, "hist.txt")
        .await
        .unwrap();
    let resp = f
        .client
        .post_form(
            &format!("/api2/repos/{}/file/?p=/hist.txt", f.repo_id),
            Some(&f.api_token),
            &[("operation", "revert"), ("commit_id", old_commit.as_str())],
        )
        .await;
    assert_eq!(resp.status(), 200, "revert must work once the lock is gone");
    let resp = f
        .client
        .download_file(&f.api_token, &f.repo_id, "/hist.txt")
        .await;
    assert_eq!(resp.bytes().await.unwrap().as_ref(), b"first-content");
}
