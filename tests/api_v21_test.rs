mod common;

use common::TestFixture;

/// D.1 — v2.1 Repos
#[tokio::test]
async fn test_v21_repos_list() {
    let f = TestFixture::new().await;
    let resp = f.client.get("/api/v2.1/repos/", Some(&f.api_token)).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["repos"].as_array().unwrap().len() >= 1);
}

#[tokio::test]
async fn test_v21_repos_detail() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .get(
            &format!("/api/v2.1/repos/{}/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["repo_id"], f.repo_id);
}

#[tokio::test]
async fn test_v21_repos_delete() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .delete(
            &format!("/api/v2.1/repos/{}/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
}

/// D.2 — v2.1 Dir
#[tokio::test]
async fn test_v21_dir_delete_file() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "file.txt", b"data")
        .await;
    assert!(resp.status().is_success());

    let resp = f
        .client
        .delete(
            &format!("/api/v2.1/repos/{}/file/?p=/file.txt", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    assert_eq!(status, 200, "delete failed ({}): {}", status, text);
}

#[tokio::test]
async fn test_v21_dir_list_with_thumbnails() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "a.txt", b"abc")
        .await;
    assert!(resp.status().is_success());

    let resp = f
        .client
        .get(
            &format!("/api/v2.1/repos/{}/dir/?p=/&with_thumbnail=true", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["dirent_list"].is_array());
    let entries = body["dirent_list"].as_array().unwrap();
    assert!(
        !entries.is_empty(),
        "v2.1 listing must contain the uploaded file"
    );
    assert!(
        entries.iter().any(|e| e["name"] == "a.txt"),
        "v2.1 listing must include 'a.txt', got: {:?}",
        entries
    );

    // Create subdirectory and verify listing is empty
    let resp = f
        .client
        .create_dir(&f.api_token, &f.repo_id, "/subdir")
        .await;
    assert_eq!(resp.status(), 200, "create subdir failed");

    let resp = f
        .client
        .get(
            &format!("/api/v2.1/repos/{}/dir/?p=/subdir", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let entries = body["dirent_list"].as_array().unwrap();
    assert!(
        entries.is_empty(),
        "empty subdir listing should be empty, got: {:?}",
        entries
    );
}

/// v2.1 list_dir must return entries after creating a directory.
#[tokio::test]
async fn test_v21_dir_list_after_create_dir() {
    let f = TestFixture::new().await;

    // Create a directory via API
    let resp = f
        .client
        .create_dir(&f.api_token, &f.repo_id, "/my_folder")
        .await;
    assert_eq!(resp.status(), 200);

    // List via v2.1 — must show the directory
    let resp = f
        .client
        .get(
            &format!("/api/v2.1/repos/{}/dir/?p=/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let entries = body["dirent_list"].as_array().unwrap();
    assert!(
        entries.iter().any(|e| e["name"] == "my_folder"),
        "v2.1 listing must contain 'my_folder', got: {:?}",
        entries
    );
}

/// v2.1 list_dir must return entries with Chinese directory names.
#[tokio::test]
async fn test_v21_dir_list_chinese_name() {
    let f = TestFixture::new().await;

    // Upload a file
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "readme.txt", b"hello")
        .await;
    assert_eq!(resp.status(), 200);

    // Create a directory with Chinese name (未命名文件夹 = "unnamed folder")
    let resp = f
        .client
        .create_dir(&f.api_token, &f.repo_id, "/未命名文件夹")
        .await;
    assert_eq!(resp.status(), 200, "create dir with Chinese name failed");

    // List via v2.1 — must show both the file and the Chinese-named directory
    let resp = f
        .client
        .get(
            &format!("/api/v2.1/repos/{}/dir/?p=/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let entries = body["dirent_list"].as_array().unwrap();
    assert!(
        entries.iter().any(|e| e["name"] == "未命名文件夹"),
        "v2.1 listing must contain '未命名文件夹', got: {:?}",
        entries
    );
    assert!(
        entries.iter().any(|e| e["name"] == "readme.txt"),
        "v2.1 listing must contain 'readme.txt', got: {:?}",
        entries
    );
}

/// D.4 — v2.1 Share Links
#[tokio::test]
async fn test_v21_share_links_create() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["token"].as_str().unwrap_or("").len() > 0);
}

#[tokio::test]
async fn test_v21_share_links_list() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .get("/api/v2.1/share-links/", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);
}

/// D.5 — v2.1 Upload Links
#[tokio::test]
async fn test_v21_upload_links_create() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .post_json(
            "/api/v2.1/upload-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);
}

/// D.6 — Activities
#[tokio::test]
async fn test_v21_activities_empty() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .get("/api/v2.1/activities/", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["events"].as_array().unwrap().is_empty());
}

/// D.7 — Wikis
#[tokio::test]
async fn test_v21_wikis_empty() {
    let f = TestFixture::new().await;
    let resp = f.client.get("/api/v2.1/wikis/", Some(&f.api_token)).await;
    assert_eq!(resp.status(), 200);
}

/// D.8 — Batch operations
#[tokio::test]
async fn test_v21_batch_move_empty_source() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .post_json(
            "/api/v2.1/repos/sync-batch-move-item/",
            Some(&f.api_token),
            &serde_json::json!({
                "src_repo_id": f.repo_id,
                "src_parent_dir": "/",
                "src_dirents": [],
                "dst_repo_id": f.repo_id,
                "dst_parent_dir": "/dest/",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);
}

/// D.9 — Other v2.1
#[tokio::test]
async fn test_v21_file_create() {
    let f = TestFixture::new().await;

    // Upload a file first so the repo has a head commit (root dir exists)
    let up = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "dummy.txt", b"seed")
        .await;
    assert!(up.status().is_success());

    // Create an empty file
    let resp = f
        .client
        .post_json(
            &format!("/api/v2.1/repos/{}/file/", f.repo_id),
            Some(&f.api_token),
            &serde_json::json!({
                "p": "/newfile.txt",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200, "create empty file should succeed");

    // Verify file exists via detail endpoint
    let detail = f
        .client
        .get(
            &format!("/api2/repos/{}/file/detail/?p=/newfile.txt", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(detail.status(), 200, "file should exist after creation");
    let body: serde_json::Value = detail.json().await.unwrap();
    assert_eq!(body["name"], "newfile.txt");
    assert_eq!(body["size"], 0, "empty file should have size 0");
    assert_eq!(body["type"], "file");

    // Create file in a subdirectory
    let mkdir = f
        .client
        .create_dir(&f.api_token, &f.repo_id, "/subdir")
        .await;
    assert_eq!(mkdir.status(), 200);

    let resp2 = f
        .client
        .post_json(
            &format!("/api/v2.1/repos/{}/file/", f.repo_id),
            Some(&f.api_token),
            &serde_json::json!({
                "p": "/subdir/child.txt",
            }),
        )
        .await;
    assert_eq!(resp2.status(), 200, "create file in subdir");

    let detail2 = f
        .client
        .get(
            &format!("/api2/repos/{}/file/detail/?p=/subdir/child.txt", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(detail2.status(), 200, "child file should exist");
}

#[tokio::test]
async fn test_v21_unauthorized() {
    let server = common::TestServer::start().await;
    let client = server.client();
    for path in &[
        "/api/v2.1/repos/",
        "/api/v2.1/share-links/",
        "/api/v2.1/upload-links/",
        "/api/v2.1/activities/",
        "/api/v2.1/wikis/",
    ] {
        let resp = client.get(path, None).await;
        assert_eq!(resp.status(), 401, "{} should be 401", path);
    }
}

/// D.7.2 — GET /api/v2.1/wikis2/
#[tokio::test]
async fn test_v21_wikis2_empty() {
    let f = TestFixture::new().await;
    let resp = f.client.get("/api/v2.1/wikis2/", Some(&f.api_token)).await;
    assert_eq!(resp.status(), 200);
}

/// D.9 — Custom share permissions
#[tokio::test]
async fn test_v21_custom_share_permissions() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .get(
            &format!("/api/v2.1/repos/{}/custom-share-permissions/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
}

/// G.2 — SDoc upload image
#[tokio::test]
async fn test_v21_seadoc_upload_image() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .post_form(
            &format!("/api/v2.1/seadoc/upload-image/{}/", uuid::Uuid::new_v4()),
            Some(&f.api_token),
            &[],
        )
        .await;
    assert_eq!(resp.status(), 200);
}
