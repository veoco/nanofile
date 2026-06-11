mod common;

use common::TestFixture;

/// D.1 — v2.1 Repos
#[tokio::test]
async fn test_v21_repos_list() {
    let f = TestFixture::new().await;
    let resp = f.client.get("/api/v2.1/repos/", Some(&f.api_token)).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(!body["repos"].as_array().unwrap().is_empty());
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

/// Regression: DELETE /api/v2.1/repos/{repo_id}/dir/?p=path must return 200
/// (Android client sends this to delete a folder; a 405 would be a regression).
#[tokio::test]
async fn test_v21_dir_delete_directory() {
    let f = TestFixture::new().await;

    // Create a directory.
    let resp = f
        .client
        .create_dir(&f.api_token, &f.repo_id, "/dir_to_delete")
        .await;
    assert_eq!(resp.status(), 200);

    // Delete via the v2.1 DELETE endpoint.
    let resp = f
        .client
        .delete(
            &format!("/api/v2.1/repos/{}/dir/?p=/dir_to_delete", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    assert_eq!(status, 200, "delete dir failed ({}): {}", status, text);

    // Verify the directory no longer exists.
    let resp = f
        .client
        .get(
            &format!("/api/v2.1/repos/{}/dir/?p=/dir_to_delete", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert!(
        !resp.status().is_success(),
        "deleted dir listing should fail, got status {}",
        resp.status()
    );
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
    assert!(!body["token"].as_str().unwrap_or("").is_empty());
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

/// v2.1 file create via multipart/form-data (simulating Android client).
#[tokio::test]
async fn test_v21_file_create_multipart() {
    let f = TestFixture::new().await;

    // Upload a file first so the repo has a head commit
    let up = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "dummy.txt", b"seed")
        .await;
    assert!(up.status().is_success());

    // Create an empty file using multipart body (Android client pattern:
    // @Multipart + @PartMap).
    use reqwest::multipart::Form;
    let form = Form::new().text("operation", "mkfile");
    let path = format!(
        "{}/api/v2.1/repos/{}/file/?p=/multipart-created.txt",
        f.server.base_url, f.repo_id
    );
    let resp = reqwest::Client::new()
        .post(&path)
        .bearer_auth(&f.api_token)
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "multipart file create failed");

    // Verify file exists
    let detail = f
        .client
        .get(
            &format!(
                "/api2/repos/{}/file/detail/?p=/multipart-created.txt",
                f.repo_id
            ),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(
        detail.status(),
        200,
        "file should exist after multipart create"
    );
}

/// GET /api/v2.1/repos/{repo_id}/dir/detail/?path=...
#[tokio::test]
async fn test_v21_dir_detail_returns_metadata() {
    let f = TestFixture::new().await;

    // Seed: upload a file so the repo has a head commit
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", ".seed", b".")
        .await;
    assert!(resp.status().is_success());

    // Create a subdirectory
    let resp = f
        .client
        .create_dir(&f.api_token, &f.repo_id, "/mydir")
        .await;
    assert_eq!(resp.status(), 200);

    // Get directory detail
    let resp = f
        .client
        .get(
            &format!("/api/v2.1/repos/{}/dir/detail/?path=/mydir", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["repo_id"], f.repo_id);
    assert_eq!(body["name"], "mydir");
    assert_eq!(body["path"], "/mydir");
    assert!(body["mtime"].as_i64().unwrap_or(0) > 0);
    assert!(body["permission"].as_str().unwrap_or("") == "rw");

    // Root path should return 400
    let resp = f
        .client
        .get(
            &format!("/api/v2.1/repos/{}/dir/detail/?path=/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 400, "root path should be rejected");

    // Missing path should return 400
    let resp = f
        .client
        .get(
            &format!("/api/v2.1/repos/{}/dir/detail/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 400, "missing path should be rejected");

    // Nonexistent path should return 404
    let resp = f
        .client
        .get(
            &format!(
                "/api/v2.1/repos/{}/dir/detail/?path=/nonexistent",
                f.repo_id
            ),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 404, "nonexistent path should 404");
}

/// Simulate the seadroid photo-backup flow and verify the v2.1 directory
/// listing response contains all fields expected by the Seafile mobile clients.
///
/// Flow: multipart mkdir → upload-blks (block upload + commit) → v2.1 GET.
#[tokio::test]
async fn test_v21_dir_list_after_photo_backup_flow() {
    let f = TestFixture::new().await;

    // ── 1. Seed: upload a file so the repo has a valid head commit ──────
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", ".seed", b".")
        .await;
    assert!(resp.status().is_success(), "seed upload failed");

    // ── 2. Create a directory via multipart POST (like seadroid mkdirRemote) ──
    let resp = f
        .client
        .create_dir_multipart(&f.api_token, &f.repo_id, "/photos")
        .await;
    assert_eq!(
        resp.status(),
        200,
        "multipart mkdir failed: {:?}",
        resp.text().await
    );

    // ── 3. Upload a block and commit it (simulating seadroid upload-blks) ──
    let file_data = b"This is a test photo file content";
    let file_size: i64 = file_data.len() as i64;

    // Compute SHA1 block id
    let block_id = {
        use sha1::{Digest, Sha1};
        let mut hasher = Sha1::new();
        hasher.update(file_data);
        hex::encode(hasher.finalize())
    };

    // Get upload-blks link
    let link_resp = f.client.upload_blks_link(&f.api_token, &f.repo_id).await;
    assert_eq!(link_resp.status(), 200);
    let upload_url: String = link_resp.json().await.unwrap();
    assert!(!upload_url.is_empty(), "upload URL should not be empty");

    // Upload block to the URL
    let block_part =
        reqwest::multipart::Part::bytes(file_data.to_vec()).file_name(block_id.clone());
    let form = reqwest::multipart::Form::new().part("file", block_part);
    let resp = f.client.post_multipart_url(&upload_url, form).await;
    assert_eq!(
        resp.status(),
        200,
        "block upload failed: {:?}",
        resp.text().await
    );

    // Commit the file
    let blockids_json = serde_json::to_string(&vec![block_id]).unwrap();
    let commit_form = reqwest::multipart::Form::new()
        .text("commitonly", "true")
        .text("parent_dir", "/photos")
        .text("file_name", "IMG_2024.jpg")
        .text("blockids", blockids_json)
        .text("file_size", file_size.to_string());
    let resp = f.client.post_multipart_url(&upload_url, commit_form).await;
    assert_eq!(
        resp.status(),
        200,
        "upload-blks commit failed: {:?}",
        resp.text().await
    );

    // ── 4. List the root directory via v2.1 GET ─────────────────────
    let resp = f
        .client
        .get(
            &format!("/api/v2.1/repos/{}/dir/?p=/&with_thumbnail=true", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let root_body: serde_json::Value = resp.json().await.unwrap();
    let root_entries = root_body["dirent_list"].as_array().unwrap();

    eprintln!(
        "=== Root directory listing ===\n{}",
        serde_json::to_string_pretty(&root_body).unwrap()
    );

    // ── 5. Verify root-level fields ──────────────────────────────────
    assert!(root_body.get("user_perm").is_some(), "missing user_perm");
    assert!(root_body.get("dir_id").is_some(), "missing dir_id");
    assert!(
        !root_entries.is_empty(),
        "root dirent_list should not be empty"
    );

    // ── 6. Verify the "photos" directory entry in root ───────────────
    let photos = root_entries.iter().find(|e| e["name"] == "photos").unwrap();
    assert_eq!(photos["type"], "dir", "photos should be a directory");
    assert!(
        photos
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .len()
            >= 40,
        "dir id should be a SHA1"
    );
    assert!(
        photos.get("mtime").and_then(|v| v.as_i64()).unwrap_or(0) > 0,
        "dir mtime should be positive"
    );
    assert_eq!(photos["permission"], "rw");
    assert_eq!(photos["parent_dir"], "/");
    assert_eq!(photos["starred"], false);
    // Directories must NOT have a "size" field (seahub compatibility)
    assert!(
        photos.get("size").is_none(),
        "directory entry should NOT have size field"
    );

    // ── 7. List /photos/ directory to verify the uploaded file ─────
    let resp = f
        .client
        .get(
            &format!(
                "/api/v2.1/repos/{}/dir/?p=/photos&with_thumbnail=true",
                f.repo_id
            ),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let photos_body: serde_json::Value = resp.json().await.unwrap();
    let photos_entries = photos_body["dirent_list"].as_array().unwrap();

    eprintln!(
        "=== /photos/ directory listing ===\n{}",
        serde_json::to_string_pretty(&photos_body).unwrap()
    );

    // ── 8. Verify the file entry in /photos/ ────────────────────────
    let file_entry = photos_entries
        .iter()
        .find(|e| e["name"] == "IMG_2024.jpg")
        .expect("IMG_2024.jpg should be in /photos/ listing");
    assert_eq!(file_entry["type"], "file");
    assert!(
        file_entry
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .len()
            >= 40,
        "file id should be a SHA1"
    );
    assert_eq!(file_entry["size"], file_size);
    assert!(
        file_entry
            .get("mtime")
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
            > 0,
        "file mtime should be positive"
    );
    assert_eq!(file_entry["permission"], "rw");
    // parent_dir must have trailing slash for non-root (seahub protocol contract)
    assert_eq!(file_entry["parent_dir"], "/photos/");
    assert_eq!(file_entry["starred"], false);
    // File must have modifier fields
    assert!(
        file_entry.get("modifier_email").is_some(),
        "missing modifier_email"
    );
    assert!(
        file_entry.get("modifier_name").is_some(),
        "missing modifier_name"
    );
    assert!(
        file_entry.get("modifier_contact_email").is_some(),
        "missing modifier_contact_email"
    );
}

/// Regression: multipart mkdir via the v2 API must return the JSON string
/// "success" because Android's SupportResponseConverter uses
/// TypeAdapter<String>.fromJson() for Call<String> responses, which throws
/// on a JSON object (it expects a JSON string literal like "\"success\"").
#[tokio::test]
async fn test_v21_mkdir_response_format() {
    let f = TestFixture::new().await;

    // Seed so the repo has a head commit.
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", ".seed_mkdir_resp", b".")
        .await;
    assert!(resp.status().is_success());

    // Root-level dir.
    let resp = f
        .client
        .create_dir_multipart(&f.api_token, &f.repo_id, "/resp_format_test")
        .await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body,
        serde_json::Value::String("success".to_string()),
        "mkdir must return the JSON string \"success\" (SupportResponseConverter)"
    );

    // Subdirectory (single level, parent already exists).
    let resp = f
        .client
        .create_dir_multipart(&f.api_token, &f.repo_id, "/resp_format_test/child")
        .await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body,
        serde_json::Value::String("success".to_string()),
        "subdir mkdir must also return \"success\""
    );
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

/// Test block download link API endpoint (GET /api2/repos/{repo_id}/files/{file_id}/blks/{block_id}/download-link/).
#[tokio::test]
async fn test_v21_block_download_link() {
    let f = TestFixture::new().await;

    // Upload a file to create a block.
    let data = b"hello block download test data";
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "test.txt", data)
        .await;
    assert!(resp.status().is_success());

    // Get file detail to find the file's fs_id.
    let detail = f
        .client
        .get(
            &format!("/api2/repos/{}/file/detail/?p=/test.txt", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(detail.status(), 200);
    let detail_body: serde_json::Value = detail.json().await.unwrap();
    let file_id = detail_body["id"].as_str().unwrap().to_string();

    // Compute the block ID (SHA1 of the data, matching test upload logic).
    let block_id = {
        use sha1::{Digest, Sha1};
        let mut hasher = Sha1::new();
        hasher.update(data);
        hex::encode(hasher.finalize())
    };

    // Call the block download link API.
    let resp = f
        .client
        .get(
            &format!(
                "/api2/repos/{}/files/{}/blks/{}/download-link/",
                f.repo_id, file_id, block_id
            ),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200, "block download link should return 200");
    let body: serde_json::Value = resp.json().await.unwrap();
    let url = body.as_str().unwrap_or("");
    assert!(
        !url.is_empty(),
        "block download link should be a non-empty URL"
    );
    assert!(url.contains(&block_id), "URL should contain block_id");

    // Unauthorized access should fail.
    let resp = f
        .client
        .get(
            &format!(
                "/api2/repos/{}/files/{}/blks/{}/download-link/",
                f.repo_id, file_id, block_id
            ),
            None,
        )
        .await;
    assert_eq!(
        resp.status(),
        401,
        "block download link without auth should 401"
    );
}
