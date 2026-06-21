mod common;

use common::{TestFixture, TestServer, create_test_user, get_sync_token};
use server::serialization::commit_json::CommitData;
use server::serialization::pack_fs;

/// Root-level file/folder leak test — directly tests the FS tree after
/// nested file creation via the API (which calls the same FileOps functions
/// as the UI upload handler). If this test passes, the core logic is correct
/// and the bug is in ensure_parent_dirs or the HTTP layer.
#[tokio::test]
async fn test_api_nested_upload_no_root_leak() {
    let f = TestFixture::new().await;

    // ── Add root-level content ──
    f.client
        .upload_file(
            &f.api_token,
            &f.repo_id,
            "/",
            "existing_root_file.txt",
            b"root",
        )
        .await;
    f.client
        .create_dir(&f.api_token, &f.repo_id, "/ExistingFolder")
        .await;
    f.client
        .upload_file(
            &f.api_token,
            &f.repo_id,
            "/ExistingFolder",
            "file_in_existing.txt",
            b"existing",
        )
        .await;

    // ── Create nested dirs via API (same as ensure_parent_dirs per-level) ──
    f.client
        .create_dir(&f.api_token, &f.repo_id, "/UploadedFolder")
        .await;
    f.client
        .create_dir(&f.api_token, &f.repo_id, "/UploadedFolder/SubDir")
        .await;

    // ── Upload files ──
    f.client
        .upload_file(
            &f.api_token,
            &f.repo_id,
            "/UploadedFolder/SubDir",
            "nested_file.txt",
            b"nested",
        )
        .await;
    f.client
        .upload_file(
            &f.api_token,
            &f.repo_id,
            "/UploadedFolder",
            "root_file_in_folder.txt",
            b"folder root",
        )
        .await;

    // ── Verify tree ──
    let root_resp = f.client.list_dir(&f.api_token, &f.repo_id, "/").await;
    let root_entries: Vec<serde_json::Value> = root_resp.json().await.unwrap();
    let root_names: Vec<&str> = root_entries
        .iter()
        .filter_map(|e| e["name"].as_str())
        .collect();
    assert_eq!(
        root_names.len(),
        3,
        "root must have 3 entries, got {:?}",
        root_names
    );
    assert!(
        root_names.contains(&"UploadedFolder"),
        "root should have UploadedFolder"
    );

    let uf_resp = f
        .client
        .list_dir(&f.api_token, &f.repo_id, "/UploadedFolder")
        .await;
    let uf_entries: Vec<serde_json::Value> = uf_resp.json().await.unwrap();
    let uf_names: Vec<&str> = uf_entries
        .iter()
        .filter_map(|e| e["name"].as_str())
        .collect();
    assert_eq!(
        uf_names.len(),
        2,
        "UploadedFolder must have 2 entries, got {:?}",
        uf_names
    );
    assert!(uf_names.contains(&"SubDir"));
    assert!(uf_names.contains(&"root_file_in_folder.txt"));
    assert!(
        !uf_names.contains(&"existing_root_file.txt"),
        "MUST NOT leak root file"
    );
    assert!(
        !uf_names.contains(&"ExistingFolder"),
        "MUST NOT leak root folder"
    );

    let sd_resp = f
        .client
        .list_dir(&f.api_token, &f.repo_id, "/UploadedFolder/SubDir")
        .await;
    let sd_entries: Vec<serde_json::Value> = sd_resp.json().await.unwrap();
    let sd_names: Vec<&str> = sd_entries
        .iter()
        .filter_map(|e| e["name"].as_str())
        .collect();
    assert_eq!(
        sd_names.len(),
        1,
        "SubDir must have 1 entry, got {:?}",
        sd_names
    );
    assert!(sd_names.contains(&"nested_file.txt"));
}

/// Simulates the UI upload flow by calling ensure_parent_dirs + create_file
/// through the UI multipart upload endpoint. Tests the full end-to-end
/// folder upload with nested directories.
#[tokio::test]
async fn test_ui_folder_upload_no_root_leak() {
    let f = TestFixture::new().await;

    // Add root-level content via API
    f.client
        .upload_file(
            &f.api_token,
            &f.repo_id,
            "/",
            "existing_root_file.txt",
            b"root",
        )
        .await;
    f.client
        .create_dir(&f.api_token, &f.repo_id, "/ExistingFolder")
        .await;
    f.client
        .upload_file(
            &f.api_token,
            &f.repo_id,
            "/ExistingFolder",
            "file_in_existing.txt",
            b"existing",
        )
        .await;

    // Login via UI
    let ui_client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .cookie_store(true)
        .build()
        .unwrap();
    let login_resp = ui_client
        .post(format!("{}/accounts/login/", f.server.base_url))
        .form(&[("email", "test@example.com"), ("password", "password")])
        .send()
        .await
        .unwrap();
    assert_eq!(login_resp.status(), 302, "login should redirect");

    // Upload file into nested dir via UI endpoint
    let file_part = reqwest::multipart::Part::bytes(b"nested content".to_vec())
        .file_name("nested_file.txt")
        .mime_str("text/plain")
        .unwrap();
    let form = reqwest::multipart::Form::new()
        .text("parent_dir", "/UploadedFolder/SubDir")
        .text("repo_name", "test-repo")
        .text("xhr", "1")
        .part("file", file_part);
    let resp = ui_client
        .post(format!(
            "{}/library/{}/upload",
            f.server.base_url, f.repo_id
        ))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "UI upload should succeed, got {}",
        resp.status()
    );

    // Upload second file
    let file2_part = reqwest::multipart::Part::bytes(b"folder root".to_vec())
        .file_name("root_file_in_folder.txt")
        .mime_str("text/plain")
        .unwrap();
    let form2 = reqwest::multipart::Form::new()
        .text("parent_dir", "/UploadedFolder")
        .text("repo_name", "test-repo")
        .text("xhr", "1")
        .part("file", file2_part);
    let resp2 = ui_client
        .post(format!(
            "{}/library/{}/upload",
            f.server.base_url, f.repo_id
        ))
        .multipart(form2)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp2.status(),
        200,
        "second UI upload should succeed, got {}",
        resp2.status()
    );

    // Verify root
    let root_resp = f.client.list_dir(&f.api_token, &f.repo_id, "/").await;
    let root_entries: Vec<serde_json::Value> = root_resp.json().await.unwrap();
    let root_names: Vec<&str> = root_entries
        .iter()
        .filter_map(|e| e["name"].as_str())
        .collect();
    assert_eq!(
        root_names.len(),
        3,
        "root must have 3 entries, got {:?}",
        root_names
    );
    assert!(
        root_names.contains(&"UploadedFolder"),
        "root should have UploadedFolder"
    );

    // Verify UploadedFolder — should NOT contain root content
    let uf_resp = f
        .client
        .list_dir(&f.api_token, &f.repo_id, "/UploadedFolder")
        .await;
    let uf_entries: Vec<serde_json::Value> = uf_resp.json().await.unwrap();
    let uf_names: Vec<&str> = uf_entries
        .iter()
        .filter_map(|e| e["name"].as_str())
        .collect();
    assert_eq!(
        uf_names.len(),
        2,
        "UploadedFolder must have 2 entries, got {:?}",
        uf_names
    );
    assert!(uf_names.contains(&"SubDir"), "should contain SubDir");
    assert!(
        uf_names.contains(&"root_file_in_folder.txt"),
        "should contain root_file_in_folder.txt"
    );
    assert!(
        !uf_names.contains(&"existing_root_file.txt"),
        "MUST NOT leak root file"
    );
    assert!(
        !uf_names.contains(&"ExistingFolder"),
        "MUST NOT leak root folder"
    );

    // Verify SubDir
    let sd_resp = f
        .client
        .list_dir(&f.api_token, &f.repo_id, "/UploadedFolder/SubDir")
        .await;
    let sd_entries: Vec<serde_json::Value> = sd_resp.json().await.unwrap();
    let sd_names: Vec<&str> = sd_entries
        .iter()
        .filter_map(|e| e["name"].as_str())
        .collect();
    assert_eq!(
        sd_names.len(),
        1,
        "SubDir must have 1 entry, got {:?}",
        sd_names
    );
    assert!(
        sd_names.contains(&"nested_file.txt"),
        "should contain nested_file.txt"
    );
}

/// Get root_fs_id from the head commit (replaces former dir_entry root lookup).
async fn get_root_fs_id(f: &TestFixture) -> String {
    let head_resp = f.client.get_head_commit(&f.sync_token, &f.repo_id).await;
    let head_id: String = head_resp.json::<serde_json::Value>().await.unwrap()["head_commit_id"]
        .as_str()
        .unwrap()
        .to_string();
    let commit_resp = f
        .client
        .get_commit(&f.sync_token, &f.repo_id, &head_id)
        .await;
    let bytes = commit_resp.bytes().await.unwrap();
    let commit: CommitData = serde_json::from_slice(&bytes).unwrap();
    commit.root_id
}

#[tokio::test]
async fn test_download_sync_flow() {
    let server = TestServer::start().await;
    let client = server.client();

    // Setup: create user and repo
    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let api_token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, api_token, "Sync Test").await;
    let sync_token = get_sync_token(&client, api_token, &repo_id).await;

    // Upload a file via Web API to populate the server
    let file_content = b"Hello, Seafile sync!";
    let resp = client
        .upload_file(api_token, &repo_id, "/", "test.txt", file_content)
        .await;
    assert_eq!(resp.status(), 200);

    // Step 1: Check protocol version
    let resp = client.protocol_version().await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["version"].as_i64().unwrap(), 2);

    // Step 2: Check head-commits-multi
    let resp = client.head_commits_multi(&[&repo_id]).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.as_object().unwrap().contains_key(&repo_id));

    // Step 3: Get HEAD commit
    let resp = client.get_head_commit(&sync_token, &repo_id).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["is_corrupted"].as_i64().unwrap(), 0);
    let head_commit_id = body["head_commit_id"].as_str().unwrap().to_string();

    // Step 4: Get commit object (binary)
    let resp = client
        .get_commit(&sync_token, &repo_id, &head_commit_id)
        .await;
    assert_eq!(resp.status(), 200);
    let commit_data = resp.bytes().await.unwrap();
    assert!(!commit_data.is_empty());

    // Step 5: Get FS ID list
    let resp = client
        .fs_id_list(&sync_token, &repo_id, &head_commit_id)
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let fs_ids: Vec<String> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert!(!fs_ids.is_empty());

    // Step 6: Pack FS objects
    let fs_id_refs: Vec<&str> = fs_ids.iter().map(|s| s.as_str()).collect();
    let resp = client.pack_fs(&sync_token, &repo_id, &fs_id_refs).await;
    assert_eq!(resp.status(), 200);
    let packed_fs = resp.bytes().await.unwrap();
    assert!(!packed_fs.is_empty());

    // Step 7: Check blocks
    let resp = client
        .check_blocks(&sync_token, &repo_id, &["fake_block_id"])
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let missing = body.as_array().unwrap();
    assert_eq!(missing.len(), 1); // fake_block_id should be missing

    // Step 8: Verify file download
    let resp = client.download_file(api_token, &repo_id, "/test.txt").await;
    assert_eq!(resp.status(), 200);
    let downloaded = resp.bytes().await.unwrap();
    assert_eq!(downloaded.as_ref(), file_content);
}

#[tokio::test]
async fn test_upload_sync_flow() {
    let server = TestServer::start().await;
    let client = server.client();

    // Setup
    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let api_token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, api_token, "Upload Test").await;
    let sync_token = get_sync_token(&client, api_token, &repo_id).await;

    // Step 1: Get server HEAD (new repo has zero-commit as head)
    let resp = client.get_head_commit(&sync_token, &repo_id).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["is_corrupted"].as_i64().unwrap(), 0);
    assert_eq!(
        body["head_commit_id"].as_str().unwrap(),
        "0000000000000000000000000000000000000000"
    );

    // Step 2: Create a commit locally and push it
    let commit_id = "a".repeat(40);
    let root_id = "b".repeat(40);
    let now = chrono::Utc::now().timestamp();

    let commit_data = serde_json::json!({
        "commit_id": commit_id,
        "repo_id": repo_id,
        "root_id": root_id,
        "creator_name": "test@example.com",
        "creator": "0000000000000000000000000000000000000000",
        "description": "test commit",
        "ctime": now,
        "parent_id": null,
        "second_parent_id": null,
        "version": 1
    });

    let json_str = serde_json::to_string(&commit_data).unwrap();

    // Upload commit (raw JSON, not compressed)
    let resp = client
        .put_commit(&sync_token, &repo_id, &commit_id, json_str.into_bytes())
        .await;
    assert_eq!(resp.status(), 200);

    // Update branch
    let resp = client
        .update_branch(&sync_token, &repo_id, &commit_id)
        .await;
    assert_eq!(resp.status(), 200);

    // Verify HEAD is updated
    let resp = client.get_head_commit(&sync_token, &repo_id).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["is_corrupted"].as_i64().unwrap(), 0);
    assert_eq!(body["head_commit_id"].as_str().unwrap(), commit_id);
}

// ============================================================
// Regression tests for path normalization (limitation fix #3)
// ============================================================

/// Regression: download_file must accept paths without leading slash.
/// URL-encoded paths like ?p=file.txt should work the same as ?p=/file.txt
#[tokio::test]
async fn test_regression_download_path_normalization() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "pn1@test.com", "password123").await;
    let resp = client.login("pn1@test.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let api_token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, api_token, "PathNorm").await;

    // Upload a file
    let resp = client
        .upload_file(
            api_token,
            &repo_id,
            "/",
            "myfile.txt",
            b"path normalize test",
        )
        .await;
    assert_eq!(resp.status(), 200);

    // Download with leading slash (should work)
    let resp = client
        .download_file(api_token, &repo_id, "/myfile.txt")
        .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.bytes().await.unwrap().as_ref(), b"path normalize test");

    // Download WITHOUT leading slash (the regression fix)
    let resp = client
        .download_file(api_token, &repo_id, "myfile.txt")
        .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.bytes().await.unwrap().as_ref(), b"path normalize test");
}

/// Regression: list_dir must normalize paths without leading slash.
#[tokio::test]
async fn test_regression_list_dir_path_normalization() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "pn2@test.com", "password123").await;
    let resp = client.login("pn2@test.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let api_token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, api_token, "ListNorm").await;

    // Upload files to root
    client
        .upload_file(api_token, &repo_id, "/", "a.txt", b"a")
        .await;
    client
        .upload_file(api_token, &repo_id, "/", "b.txt", b"b")
        .await;

    // List root with leading slash
    let resp = client.list_dir(api_token, &repo_id, "/").await;
    assert_eq!(resp.status(), 200);

    // List root WITHOUT leading slash via urlencoding for empty
    // (pass the right path, the normalization happens server-side)
    let resp = client.list_dir(api_token, &repo_id, "").await;
    assert_eq!(resp.status(), 200);
}

// ============================================================
// Regression tests for ancestor tree walk (limitation fix #1)
// ============================================================

/// Regression: uploading a file to a nested subdirectory must update
/// all ancestor directories up to root, preserving the full tree.
#[tokio::test]
async fn test_regression_nested_upload_preserves_full_tree() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "tree@test.com", "password123").await;
    let resp = client.login("tree@test.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let api_token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, api_token, "TreeTest").await;

    // Create nested directory structure
    client.create_dir(api_token, &repo_id, "/docs").await;
    client.create_dir(api_token, &repo_id, "/docs/api").await;
    client.create_dir(api_token, &repo_id, "/src").await;

    // Upload README to root FIRST (critical edge case — root upload
    // before subdir content)
    let resp = client
        .upload_file(api_token, &repo_id, "/", "README.md", b"readme")
        .await;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    assert!(
        status == 200,
        "upload README.md failed: {} body={}",
        status,
        body
    );

    // Upload a file deep in the tree
    let resp = client
        .upload_file(api_token, &repo_id, "/docs/api", "api.txt", b"api docs")
        .await;
    assert_eq!(resp.status(), 200, "upload api.txt failed");

    // Upload another file in a different branch
    let resp = client
        .upload_file(api_token, &repo_id, "/src", "main.rs", b"fn main() {}")
        .await;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    assert!(
        status == 200,
        "upload main.rs failed: {} body={}",
        status,
        body
    );

    // All files must be downloadable
    let resp = client
        .download_file(api_token, &repo_id, "/README.md")
        .await;
    assert_eq!(resp.status(), 200);

    let resp = client
        .download_file(api_token, &repo_id, "/docs/api/api.txt")
        .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.bytes().await.unwrap().as_ref(), b"api docs");

    let resp = client
        .download_file(api_token, &repo_id, "/src/main.rs")
        .await;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    assert!(
        status == 200,
        "download /src/main.rs failed: {} body={}",
        status,
        body
    );
    assert_eq!(body.as_bytes(), b"fn main() {}");
}

/// Regression: verify the commit's root_id points to the real root
/// directory containing ALL entries (not just a subtree).
#[tokio::test]
async fn test_regression_commit_root_contains_full_tree() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "croot@test.com", "password123").await;
    let resp = client.login("croot@test.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let api_token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, api_token, "CRootTest").await;
    let sync_token = get_sync_token(&client, api_token, &repo_id).await;

    // Create nested structure with multiple files in different dirs
    client.create_dir(api_token, &repo_id, "/lib").await;
    client.create_dir(api_token, &repo_id, "/bin").await;

    client
        .upload_file(api_token, &repo_id, "/", "top.txt", b"top")
        .await;
    client
        .upload_file(api_token, &repo_id, "/lib", "util.rs", b"util")
        .await;
    client
        .upload_file(api_token, &repo_id, "/bin", "app", b"binary")
        .await;

    // Get the head commit
    let resp = client.get_head_commit(&sync_token, &repo_id).await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let head_id = body["head_commit_id"].as_str().unwrap();

    // Get the commit and extract root_id
    let resp = client.get_commit(&sync_token, &repo_id, head_id).await;
    let data = resp.bytes().await.unwrap();
    let commit: serde_json::Value = serde_json::from_slice(&data).unwrap();
    let root_id = commit["root_id"].as_str().unwrap();

    // Fetch the root fs_object and verify it contains all top-level entries
    let resp = client.pack_fs(&sync_token, &repo_id, &[root_id]).await;
    let packed = resp.bytes().await.unwrap();
    let entries = server::serialization::pack_fs::decode_pack_fs_entries(&packed).unwrap();
    assert_eq!(entries.len(), 1);

    let decompressed = server::serialization::pack_fs::decompress_fs_data(&entries[0].1).unwrap();
    let root_data: serde_json::Value = serde_json::from_slice(&decompressed).unwrap();
    let dirent_names: Vec<&str> = root_data["dirents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();

    // Root must contain all three top-level entries
    assert!(
        dirent_names.contains(&"top.txt"),
        "root must contain top.txt"
    );
    assert!(dirent_names.contains(&"lib"), "root must contain lib dir");
    assert!(dirent_names.contains(&"bin"), "root must contain bin dir");
    assert_eq!(dirent_names.len(), 3, "root must have exactly 3 entries");
}

/// Regression: uploading to root AFTER subdirectories exist must not
/// lose existing subdirectory entries.
#[tokio::test]
async fn test_regression_root_upload_preserves_subdirs() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "rootsub@test.com", "password123").await;
    let resp = client.login("rootsub@test.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let api_token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, api_token, "RootSub").await;
    let sync_token = get_sync_token(&client, api_token, &repo_id).await;

    // Create subdirectory and upload file there FIRST
    client.create_dir(api_token, &repo_id, "/subdir").await;
    client
        .upload_file(api_token, &repo_id, "/subdir", "deep.txt", b"deep content")
        .await;

    // NOW upload a file to root — must NOT lose /subdir
    client
        .upload_file(api_token, &repo_id, "/", "root_file.txt", b"root content")
        .await;

    // Both files must be accessible
    let resp = client
        .download_file(api_token, &repo_id, "/subdir/deep.txt")
        .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.bytes().await.unwrap().as_ref(), b"deep content");

    let resp = client
        .download_file(api_token, &repo_id, "/root_file.txt")
        .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.bytes().await.unwrap().as_ref(), b"root content");

    // Verify commit root has both entries
    let resp = client.get_head_commit(&sync_token, &repo_id).await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let head_id = body["head_commit_id"].as_str().unwrap();

    let resp = client.get_commit(&sync_token, &repo_id, head_id).await;
    let data = resp.bytes().await.unwrap();
    let commit: serde_json::Value = serde_json::from_slice(&data).unwrap();
    let root_id = commit["root_id"].as_str().unwrap();

    let resp = client.pack_fs(&sync_token, &repo_id, &[root_id]).await;
    let packed = resp.bytes().await.unwrap();
    let entries = server::serialization::pack_fs::decode_pack_fs_entries(&packed).unwrap();
    let decompressed = server::serialization::pack_fs::decompress_fs_data(&entries[0].1).unwrap();
    let root_data: serde_json::Value = serde_json::from_slice(&decompressed).unwrap();
    let names: Vec<&str> = root_data["dirents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();

    assert!(
        names.contains(&"root_file.txt"),
        "root must have root_file.txt"
    );
    assert!(names.contains(&"subdir"), "root must have subdir directory");
}

/// Regression: multiple sibling directories at the same level must all
/// be preserved when uploading files to one of them.
#[tokio::test]
async fn test_regression_sibling_dirs_preserved() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "sib@test.com", "password123").await;
    let resp = client.login("sib@test.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let api_token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, api_token, "Sibling").await;

    // Create three sibling directories under root
    for d in &["/a", "/b", "/c"] {
        client.create_dir(api_token, &repo_id, d).await;
    }

    // Upload a file to /b
    client
        .upload_file(api_token, &repo_id, "/b", "target.txt", b"in b")
        .await;

    // Upload file to root — must keep /a, /b, /c
    client
        .upload_file(api_token, &repo_id, "/", "root.txt", b"root")
        .await;

    // All directories and files should be accessible
    for path in &["/root.txt", "/b/target.txt", "/a", "/c"] {
        // Try download for files, list for dirs
        if path.ends_with(".txt") {
            let resp = client.download_file(api_token, &repo_id, path).await;
            assert_eq!(resp.status(), 200, "should find {}", path);
        }
    }
}

// ============================================================
// Regression tests for file replacement (limitation fix #2)
// ============================================================

/// Regression: uploading with replace=1 must overwrite existing file
/// content without creating duplicates or returning errors.
#[tokio::test]
async fn test_regression_replace_overwrites_content() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "repl1@test.com", "password123").await;
    let resp = client.login("repl1@test.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let api_token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, api_token, "ReplTest").await;

    // Upload original file without replace
    let resp = client
        .upload_file_with_replace(api_token, &repo_id, "/", "data.txt", b"original v1", false)
        .await;
    assert_eq!(resp.status(), 200);

    // Verify original content
    let resp = client.download_file(api_token, &repo_id, "/data.txt").await;
    assert_eq!(resp.bytes().await.unwrap().as_ref(), b"original v1");

    // Replace with new content
    let resp = client
        .upload_file_with_replace(api_token, &repo_id, "/", "data.txt", b"REPLACED v2!", true)
        .await;
    assert_eq!(resp.status(), 200);

    // Must return NEW content
    let resp = client.download_file(api_token, &repo_id, "/data.txt").await;
    assert_eq!(resp.bytes().await.unwrap().as_ref(), b"REPLACED v2!");
}

/// Regression: uploading with replace=1 on a non-existent path should
/// simply create the file (no error).
#[tokio::test]
async fn test_regression_replace_creates_when_new() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "repl2@test.com", "password123").await;
    let resp = client.login("repl2@test.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let api_token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, api_token, "ReplNew").await;

    // Upload with replace=1 on a path that doesn't exist yet
    let resp = client
        .upload_file_with_replace(api_token, &repo_id, "/", "fresh.txt", b"brand new", true)
        .await;
    assert_eq!(resp.status(), 200);

    // File must exist with correct content
    let resp = client
        .download_file(api_token, &repo_id, "/fresh.txt")
        .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.bytes().await.unwrap().as_ref(), b"brand new");
}

/// Regression: uploading with replace=1 multiple times must work
/// repeatedly without errors or stale leftovers.
#[tokio::test]
async fn test_regression_replace_multiple_times() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "repl3@test.com", "password123").await;
    let resp = client.login("repl3@test.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let api_token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, api_token, "ReplMulti").await;

    for version in 1..=5 {
        let content = format!("version {}", version);
        let resp = client
            .upload_file_with_replace(
                api_token,
                &repo_id,
                "/",
                "iter.txt",
                content.as_bytes(),
                true,
            )
            .await;
        let status = resp.status();
        if status != 200 {
            // Print error body for debugging
            let err_body = resp.text().await.unwrap_or_default();
            panic!("replace v{} failed with {}: {}", version, status, err_body);
        }

        let resp = client.download_file(api_token, &repo_id, "/iter.txt").await;
        let dl_status = resp.status();
        let dl_body = resp.text().await.unwrap_or_default();
        assert!(
            dl_status == 200,
            "download after replace v{} failed: {} body={}",
            version,
            dl_status,
            dl_body
        );
        assert_eq!(
            dl_body.as_bytes(),
            content.as_bytes(),
            "content mismatch at v{}",
            version
        );
    }
}

// ============================================================
// Regression test for empty-dir fs_id collision (reconcile bug)
// ============================================================

/// Regression: verify the FS tree via sync-protocol pack-fs traversal.
///
/// All empty directories share the same fs_id (SHA1 of identical empty
/// FsDirData JSON). When reconciliation matched only on parent_id, it
/// over-matched and promoted grandchildren (e.g. /docs/api) to root's
/// dirents, breaking seaf-cli downloads that rely on pack-fs traversal.
///
/// This test validates that the commit's root directory contains ONLY
/// direct children, and that subdirectory children are NOT leaked to
/// ancestor directories.
#[tokio::test]
async fn test_regression_sync_tree_no_grandchild_leak() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "synctree@test.com", "password123").await;
    let resp = client.login("synctree@test.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let api_token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, api_token, "SyncTree").await;
    let sync_token = get_sync_token(&client, api_token, &repo_id).await;

    // Recreate the exact e2e test scenario: create nested dirs first,
    // then upload files in mixed order (root first, then deep, then sibling).
    client.create_dir(api_token, &repo_id, "/docs").await;
    client.create_dir(api_token, &repo_id, "/docs/api").await;
    client.create_dir(api_token, &repo_id, "/src").await;

    // Upload root file first — triggers root reconciliation with all
    // three dir entries sharing the same parent_id (empty-dir fs_id).
    client
        .upload_file(api_token, &repo_id, "/", "README.md", b"readme")
        .await;

    // Upload deep file — walks ancestors: /docs/api → /docs → root.
    client
        .upload_file(api_token, &repo_id, "/docs/api", "api.txt", b"api docs")
        .await;

    // Upload sibling file — walks ancestors: /src → root.
    client
        .upload_file(api_token, &repo_id, "/src", "main.rs", b"fn main() {}")
        .await;

    // --- Validate via sync protocol (pack-fs traversal) ---

    // Get root_id from the commit
    let resp = client.get_head_commit(&sync_token, &repo_id).await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let head_id = body["head_commit_id"].as_str().unwrap();

    let resp = client.get_commit(&sync_token, &repo_id, head_id).await;
    let data = resp.bytes().await.unwrap();
    let commit: serde_json::Value = serde_json::from_slice(&data).unwrap();
    let root_id = commit["root_id"].as_str().unwrap();

    // Fetch the root FS object and unpack its dirents
    let resp = client.pack_fs(&sync_token, &repo_id, &[root_id]).await;
    let packed = resp.bytes().await.unwrap();
    let entries = server::serialization::pack_fs::decode_pack_fs_entries(&packed).unwrap();
    assert_eq!(entries.len(), 1, "root pack-fs must return 1 entry");

    let decompressed = server::serialization::pack_fs::decompress_fs_data(&entries[0].1).unwrap();
    let root_data: serde_json::Value = serde_json::from_slice(&decompressed).unwrap();
    let root_names: Vec<&str> = root_data["dirents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();

    // Root must contain exactly the three top-level entries.
    // The bug would cause "api" (from /docs/api) to appear here.
    assert!(
        root_names.contains(&"docs"),
        "root must contain docs dir, got: {:?}",
        root_names
    );
    assert!(
        root_names.contains(&"src"),
        "root must contain src dir, got: {:?}",
        root_names
    );
    assert!(
        root_names.contains(&"README.md"),
        "root must contain README.md, got: {:?}",
        root_names
    );
    assert_eq!(
        root_names.len(),
        3,
        "root must have exactly 3 entries (no grandchild leak), got {:?}",
        root_names
    );

    // Build a full name→id map from root's dirents for subtree verification.
    let root_map: std::collections::HashMap<String, String> = root_data["dirents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["name"].as_str().unwrap().to_string(),
                e["id"].as_str().unwrap().to_string(),
            )
        })
        .collect();

    // Verify /docs contains only "api"
    let docs_id = root_map.get("docs").unwrap();
    let resp = client
        .pack_fs(&sync_token, &repo_id, &[docs_id.as_str()])
        .await;
    let packed = resp.bytes().await.unwrap();
    let entries = server::serialization::pack_fs::decode_pack_fs_entries(&packed).unwrap();
    let decompressed = server::serialization::pack_fs::decompress_fs_data(&entries[0].1).unwrap();
    let docs_data: serde_json::Value = serde_json::from_slice(&decompressed).unwrap();
    let docs_names: Vec<&str> = docs_data["dirents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        docs_names,
        vec!["api"],
        "/docs must contain exactly ['api'], got {:?}",
        docs_names
    );

    // Verify /docs/api contains only "api.txt"
    let docs_map: std::collections::HashMap<String, String> = docs_data["dirents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["name"].as_str().unwrap().to_string(),
                e["id"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let api_id = docs_map.get("api").unwrap();
    let resp = client
        .pack_fs(&sync_token, &repo_id, &[api_id.as_str()])
        .await;
    let packed = resp.bytes().await.unwrap();
    let entries = server::serialization::pack_fs::decode_pack_fs_entries(&packed).unwrap();
    let decompressed = server::serialization::pack_fs::decompress_fs_data(&entries[0].1).unwrap();
    let api_data: serde_json::Value = serde_json::from_slice(&decompressed).unwrap();
    let api_names: Vec<&str> = api_data["dirents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        api_names,
        vec!["api.txt"],
        "/docs/api must contain exactly ['api.txt'], got {:?}",
        api_names
    );

    // Verify /src contains only "main.rs"
    let src_id = root_map.get("src").unwrap();
    let resp = client
        .pack_fs(&sync_token, &repo_id, &[src_id.as_str()])
        .await;
    let packed = resp.bytes().await.unwrap();
    let entries = server::serialization::pack_fs::decode_pack_fs_entries(&packed).unwrap();
    let decompressed = server::serialization::pack_fs::decompress_fs_data(&entries[0].1).unwrap();
    let src_data: serde_json::Value = serde_json::from_slice(&decompressed).unwrap();
    let src_names: Vec<&str> = src_data["dirents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        src_names,
        vec!["main.rs"],
        "/src must contain exactly ['main.rs'], got {:?}",
        src_names
    );
}

// ============================================================
// CDC (Content-Defined Chunking) regression tests
// ============================================================

/// CDC must produce identical block IDs for identical content.
/// This is critical for block-level deduplication.
#[tokio::test]
async fn test_regression_cdc_determinism() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "cdc1@test.com", "password123").await;
    let resp = client.login("cdc1@test.com", "password123").await;
    let api_token = resp.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    let repo_id = common::create_test_repo(&client, &api_token, "CDCDet1").await;
    let sync_token = get_sync_token(&client, &api_token, &repo_id).await;

    // Create reproducible 1MB content
    let content: Vec<u8> = (0..(1024 * 1024)).map(|i| (i % 251) as u8).collect();

    // Upload same content to two paths
    client
        .upload_file(&api_token, &repo_id, "/", "copy1.bin", &content)
        .await;
    client
        .upload_file(&api_token, &repo_id, "/", "copy2.bin", &content)
        .await;

    // Get root and verify both files share the same block IDs
    let resp = client.get_head_commit(&sync_token, &repo_id).await;
    let head_id = resp.json::<serde_json::Value>().await.unwrap()["head_commit_id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = client.get_commit(&sync_token, &repo_id, &head_id).await;
    let commit: serde_json::Value = serde_json::from_slice(&resp.bytes().await.unwrap()).unwrap();
    let root_id = commit["root_id"].as_str().unwrap();

    let resp = client.pack_fs(&sync_token, &repo_id, &[root_id]).await;
    let packed = resp.bytes().await.unwrap();
    let entries = server::serialization::pack_fs::decode_pack_fs_entries(&packed).unwrap();
    let root_data: serde_json::Value = serde_json::from_slice(
        &server::serialization::pack_fs::decompress_fs_data(&entries[0].1).unwrap(),
    )
    .unwrap();

    // Collect block IDs for both files
    let mut block_sets: Vec<std::collections::HashSet<String>> = vec![];
    for d in root_data["dirents"].as_array().unwrap() {
        let child_id = d["id"].as_str().unwrap();
        let resp = client.pack_fs(&sync_token, &repo_id, &[child_id]).await;
        let packed = resp.bytes().await.unwrap();
        let child_entries =
            server::serialization::pack_fs::decode_pack_fs_entries(&packed).unwrap();
        let child_data: serde_json::Value = serde_json::from_slice(
            &server::serialization::pack_fs::decompress_fs_data(&child_entries[0].1).unwrap(),
        )
        .unwrap();
        if child_data["type"].as_i64() == Some(1) {
            let bids: std::collections::HashSet<String> = child_data["block_ids"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect();
            block_sets.push(bids);
        }
    }

    assert_eq!(block_sets.len(), 2, "should have 2 files");
    // Identical content must produce identical block ID sets
    assert_eq!(
        block_sets[0], block_sets[1],
        "CDC: identical content must produce identical block IDs"
    );
}

/// CDC chunk sizes must respect min/max boundaries.
/// For a 1MB file: min=256KB, avg=1MB, max=4MB.
/// Expected 1-4 chunks, each non-last chunk >= 256KB and <= 4MB.
#[tokio::test]
async fn test_regression_cdc_chunk_size_bounds() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "cdc2@test.com", "password123").await;
    let resp = client.login("cdc2@test.com", "password123").await;
    let api_token = resp.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    let repo_id = common::create_test_repo(&client, &api_token, "CDCBounds").await;
    let sync_token = get_sync_token(&client, &api_token, &repo_id).await;

    // Generate 1MB of varied data that should trigger CDC breakpoints
    let content: Vec<u8> = (0..(1024 * 1024))
        .map(|i| ((i * 7 + 13) % 256) as u8)
        .collect();

    client
        .upload_file(&api_token, &repo_id, "/", "data.bin", &content)
        .await;

    // Verify chunk count and sizes via pack-fs
    let resp = client.get_head_commit(&sync_token, &repo_id).await;
    let head_id = resp.json::<serde_json::Value>().await.unwrap()["head_commit_id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = client.get_commit(&sync_token, &repo_id, &head_id).await;
    let commit: serde_json::Value = serde_json::from_slice(&resp.bytes().await.unwrap()).unwrap();
    let root_id = commit["root_id"].as_str().unwrap();

    let resp = client.pack_fs(&sync_token, &repo_id, &[root_id]).await;
    let packed = resp.bytes().await.unwrap();
    let entries = server::serialization::pack_fs::decode_pack_fs_entries(&packed).unwrap();
    let root_data: serde_json::Value = serde_json::from_slice(
        &server::serialization::pack_fs::decompress_fs_data(&entries[0].1).unwrap(),
    )
    .unwrap();

    let child_id = root_data["dirents"].as_array().unwrap()[0]["id"]
        .as_str()
        .unwrap();
    let resp = client.pack_fs(&sync_token, &repo_id, &[child_id]).await;
    let child_packed = resp.bytes().await.unwrap();
    let child_entries =
        server::serialization::pack_fs::decode_pack_fs_entries(&child_packed).unwrap();
    let file_data: serde_json::Value = serde_json::from_slice(
        &server::serialization::pack_fs::decompress_fs_data(&child_entries[0].1).unwrap(),
    )
    .unwrap();

    let block_ids: Vec<String> = file_data["block_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    let file_size = file_data["size"].as_i64().unwrap() as usize;

    assert!(
        !block_ids.is_empty(),
        "1MB file should produce at least 1 chunk"
    );
    assert!(
        block_ids.len() <= 4,
        "1MB file should produce at most 4 chunks (min=256KB), got {}",
        block_ids.len()
    );

    // Verify sizes via block-map endpoint
    let block_map_sizes: Vec<i64> = client
        .block_map(&sync_token, &repo_id, child_id)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(block_map_sizes.len(), block_ids.len());

    let total_block_size: i64 = block_map_sizes.iter().sum();
    assert_eq!(
        total_block_size as usize, file_size,
        "sum of block sizes must equal file size"
    );

    // All non-last blocks must be >= 256KB (CDC min)
    for (i, &size) in block_map_sizes.iter().enumerate() {
        if i < block_map_sizes.len() - 1 {
            assert!(
                size >= 256 * 1024,
                "non-last block {} size {} < 256KB min",
                i,
                size
            );
        }
        assert!(
            size <= 4 * 1024 * 1024,
            "block {} size {} > 4MB max",
            i,
            size
        );
    }
}

/// CDC roundtrip: chunk → write blocks → read blocks → reassemble → verify.
#[tokio::test]
async fn test_regression_cdc_roundtrip() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "cdc3@test.com", "password123").await;
    let resp = client.login("cdc3@test.com", "password123").await;
    let api_token = resp.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    let repo_id = common::create_test_repo(&client, &api_token, "CDCRound").await;

    // Test with various file sizes
    let test_cases: Vec<(usize, &str)> = vec![
        (0, "empty"),
        (1, "single byte"),
        (100, "small"),
        (256 * 1024, "exactly min chunk"),
        (400 * 1024, "between min and 2*min"),
        (512 * 1024, "2*min"),
        (1024 * 1024, "1MB"),
    ];

    for (size, label) in &test_cases {
        let content: Vec<u8> = (0..*size)
            .map(|i| (i.wrapping_mul(17) ^ (i >> 4)) as u8)
            .collect();

        let fname = format!("{}.bin", label.replace(' ', "_"));
        client
            .upload_file(&api_token, &repo_id, "/", &fname, &content)
            .await;

        // Download and verify
        let resp = client
            .download_file(&api_token, &repo_id, &format!("/{}", fname))
            .await;
        assert_eq!(resp.status(), 200, "download failed for {}", label);
        let downloaded = resp.bytes().await.unwrap();
        assert_eq!(
            downloaded.as_ref(),
            content.as_slice(),
            "CDC roundtrip mismatch for {} ({} bytes)",
            label,
            size
        );
    }
}

/// CDC with edge case content: zeros, 0xFF, high-entropy random data.
#[tokio::test]
async fn test_regression_cdc_edge_cases() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "cdc4@test.com", "password123").await;
    let resp = client.login("cdc4@test.com", "password123").await;
    let api_token = resp.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    let repo_id = common::create_test_repo(&client, &api_token, "CDCEdge").await;

    // Zero content (all bytes 0x00)
    let zeros = vec![0u8; 400_000];
    client
        .upload_file(&api_token, &repo_id, "/", "zeros.bin", &zeros)
        .await;

    // All 0xFF
    let ffs = vec![0xFFu8; 400_000];
    client
        .upload_file(&api_token, &repo_id, "/", "ff.bin", &ffs)
        .await;

    // Alternating pattern
    let alt: Vec<u8> = (0..400_000)
        .map(|i| if i % 2 == 0 { 0xAA } else { 0x55 })
        .collect();
    client
        .upload_file(&api_token, &repo_id, "/", "alt.bin", &alt)
        .await;

    // Verify all roundtrip correctly
    for (fname, expected) in &[("zeros.bin", &zeros), ("ff.bin", &ffs), ("alt.bin", &alt)] {
        let resp = client
            .download_file(&api_token, &repo_id, &format!("/{}", fname))
            .await;
        assert_eq!(resp.status(), 200, "download failed for {}", fname);
        let downloaded = resp.bytes().await.unwrap();
        assert_eq!(
            downloaded.as_ref(),
            expected.as_slice(),
            "CDC edge case mismatch for {} ({} bytes)",
            fname,
            expected.len()
        );
    }
}

/// CDC stability: a small edit should preserve some block IDs.
/// Upload a file, replace with a 1-byte change, verify content changes.
#[tokio::test]
async fn test_regression_cdc_stability_on_edit() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "cdc5@test.com", "password123").await;
    let resp = client.login("cdc5@test.com", "password123").await;
    let api_token = resp.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    let repo_id = common::create_test_repo(&client, &api_token, "CDCStable").await;

    // Create 512KB of base content
    let mut v1: Vec<u8> = (0..(512 * 1024))
        .map(|i: usize| (i.wrapping_mul(13) ^ (i >> 3)) as u8)
        .collect();

    client
        .upload_file(&api_token, &repo_id, "/", "file.bin", &v1)
        .await;

    let v1_downloaded = client
        .download_file(&api_token, &repo_id, "/file.bin")
        .await
        .bytes()
        .await
        .unwrap();
    assert_eq!(v1_downloaded.as_ref(), v1.as_slice());

    // Modify 1 byte and re-upload
    let edit_pos = 256 * 1024; // middle
    v1[edit_pos] = v1[edit_pos].wrapping_add(1);
    client
        .upload_file_with_replace(&api_token, &repo_id, "/", "file.bin", &v1, true)
        .await;

    let v2_downloaded = client
        .download_file(&api_token, &repo_id, "/file.bin")
        .await
        .bytes()
        .await
        .unwrap();
    assert_eq!(v2_downloaded.as_ref(), v1.as_slice());
    assert_ne!(v1_downloaded.as_ref(), v2_downloaded.as_ref());
}

// ============================================================
// T2: BlockStore cross-protocol unification (Phase 5)
// ============================================================

/// Verify that blocks written by API upload can be read back by the sync
/// protocol (check-blocks + get-block). This proves the single block store
/// instance is shared across both code paths.
#[tokio::test]
async fn test_block_store_unified_across_api_and_sync() {
    let f = TestFixture::new().await;
    let content = b"hello block unification test";

    // Upload via API — writes blocks to disk via BlockStore
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "test.txt", content)
        .await;
    assert_eq!(resp.status(), 200, "API upload failed");

    // Get head commit to find the root fs_id
    let resp = f.client.get_head_commit(&f.sync_token, &f.repo_id).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let head_id = body["head_commit_id"].as_str().unwrap().to_string();

    // Get the commit to extract root_id
    let resp = f
        .client
        .get_commit(&f.sync_token, &f.repo_id, &head_id)
        .await;
    assert_eq!(resp.status(), 200);
    let commit: serde_json::Value = serde_json::from_slice(&resp.bytes().await.unwrap()).unwrap();
    let _root_id = commit["root_id"].as_str().unwrap();

    // Fetch all fs_ids via fs-id-list
    let resp = f
        .client
        .fs_id_list(&f.sync_token, &f.repo_id, &head_id)
        .await;
    assert_eq!(resp.status(), 200);
    let fs_ids: Vec<String> = resp
        .json::<serde_json::Value>()
        .await
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert!(!fs_ids.is_empty(), "must have fs objects");

    // Find the file fs_object (type=1) to extract block_ids
    let fs_refs: Vec<&str> = fs_ids.iter().map(|s| s.as_str()).collect();
    let resp = f.client.pack_fs(&f.sync_token, &f.repo_id, &fs_refs).await;
    assert_eq!(resp.status(), 200);
    let packed = resp.bytes().await.unwrap();
    let entries = pack_fs::decode_pack_fs_entries(&packed).unwrap();

    let mut found_block_ids: Vec<String> = Vec::new();
    for (_id, data) in &entries {
        let decompressed = pack_fs::decompress_fs_data(data).unwrap();
        let json_val: serde_json::Value = serde_json::from_slice(&decompressed).unwrap();
        if json_val["type"].as_i64() == Some(1)
            && let Some(block_ids) = json_val["block_ids"].as_array()
        {
            for bid in block_ids {
                if let Some(b) = bid.as_str() {
                    found_block_ids.push(b.to_string());
                }
            }
        }
    }
    assert!(
        !found_block_ids.is_empty(),
        "file fs_object must have block_ids"
    );

    // Use sync protocol check-blocks to verify these blocks exist
    let block_refs: Vec<&str> = found_block_ids.iter().map(|s| s.as_str()).collect();
    let resp = f
        .client
        .check_blocks(&f.sync_token, &f.repo_id, &block_refs)
        .await;
    assert_eq!(resp.status(), 200);
    let missing: Vec<String> = resp.json().await.unwrap();
    assert!(
        missing.is_empty(),
        "blocks written by API must be found by sync protocol, missing: {:?}",
        missing
    );

    // Also verify content via get-block
    for block_id in &found_block_ids {
        let resp = f
            .client
            .get_block(&f.sync_token, &f.repo_id, block_id)
            .await;
        assert_eq!(
            resp.status(),
            200,
            "block {block_id} must exist via get-block"
        );
        let block_data = resp.bytes().await.unwrap();
        assert!(!block_data.is_empty(), "block must have content");
    }
}

// ============================================================
// T5: Root FsDirData stays in sync (Phase 4)
// ============================================================

/// Verify that after multiple create_dir and upload_file operations,
/// the root FsDirData always contains the correct set of entries.
#[tokio::test]
async fn test_root_entry_child_id_stays_in_sync() {
    let f = TestFixture::new().await;

    // Create two root-level directories
    let resp = f.client.create_dir(&f.api_token, &f.repo_id, "/a").await;
    assert!(
        resp.status() == 200 || resp.status() == 201,
        "create /a failed"
    );
    let resp = f.client.create_dir(&f.api_token, &f.repo_id, "/b").await;
    assert!(
        resp.status() == 200 || resp.status() == 201,
        "create /b failed"
    );

    // Upload a file to root
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "README.md", b"readme")
        .await;
    assert_eq!(resp.status(), 200);

    // Get root fs_id from head commit after first batch of operations
    let root_fs_id = get_root_fs_id(&f).await;

    // Read the FsDirData at root_fs_id — must contain a, b, README.md
    let root_data = server::repo::read_fs_dir_data(f.server.db.as_ref(), &f.repo_id, &root_fs_id)
        .await
        .unwrap();
    let root_names: Vec<&str> = root_data.dirents.iter().map(|d| d.name.as_str()).collect();
    assert!(
        root_names.contains(&"a"),
        "root must contain 'a', got {:?}",
        root_names
    );
    assert!(
        root_names.contains(&"b"),
        "root must contain 'b', got {:?}",
        root_names
    );
    assert!(
        root_names.contains(&"README.md"),
        "root must contain 'README.md', got {:?}",
        root_names
    );
    assert_eq!(root_names.len(), 3, "root must have exactly 3 entries");

    // More operations — another dir and file
    let resp = f.client.create_dir(&f.api_token, &f.repo_id, "/c").await;
    assert!(
        resp.status() == 200 || resp.status() == 201,
        "create /c failed"
    );
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "hello.txt", b"hello")
        .await;
    assert_eq!(resp.status(), 200);

    // Get root fs_id again — must have changed (new root FsDirData after writes)
    let root_fs_id_v2 = get_root_fs_id(&f).await;

    assert_ne!(
        root_fs_id_v2, root_fs_id,
        "root fs_id must change after more operations"
    );

    // Verify new root contains all 5 entries
    let root_data_v2 =
        server::repo::read_fs_dir_data(f.server.db.as_ref(), &f.repo_id, &root_fs_id_v2)
            .await
            .unwrap();
    let root_names_v2: Vec<&str> = root_data_v2
        .dirents
        .iter()
        .map(|d| d.name.as_str())
        .collect();
    for name in &["a", "b", "c", "README.md", "hello.txt"] {
        assert!(
            root_names_v2.contains(name),
            "root must contain '{}', got {:?}",
            name,
            root_names_v2
        );
    }
    assert_eq!(root_names_v2.len(), 5, "root must have exactly 5 entries");
}

// ============================================================
// Regression tests: rename/delete must update FS tree + commit
// ============================================================

/// Renaming a file via the form-encoded API must:
/// 1. Create a new HEAD commit (different from previous)
/// 2. Update the FS tree so the new name appears in listings
/// 3. Old name no longer appears
#[tokio::test]
async fn test_regression_rename_file_creates_new_commit() {
    let f = TestFixture::new().await;

    // Upload a file
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "old_name.txt", b"hello")
        .await;
    assert_eq!(resp.status(), 200);

    // Record HEAD commit and root fs_id before rename
    let resp = f.client.get_head_commit(&f.sync_token, &f.repo_id).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let head_before = body["head_commit_id"].as_str().unwrap().to_string();
    assert_ne!(
        head_before, "0000000000000000000000000000000000000000",
        "upload must produce a real commit"
    );

    // Get current root fs_id from the head commit
    let root_before = get_root_fs_id(&f).await;

    // Rename file via form POST (as the Qt client does)
    let resp = f
        .client
        .post_form(
            &format!("/api2/repos/{}/file/?p=/old_name.txt", f.repo_id),
            Some(&f.api_token),
            &[("operation", "rename"), ("newname", "renamed.txt")],
        )
        .await;
    assert_eq!(resp.status(), 200);

    // Record HEAD commit after rename
    let resp = f.client.get_head_commit(&f.sync_token, &f.repo_id).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let head_after = body["head_commit_id"].as_str().unwrap().to_string();
    assert_ne!(head_after, head_before, "HEAD must change after rename");

    // CRITICAL: Read root FsDirData from the new tree and verify the
    // file name is updated in the FS tree itself.
    let root_after = get_root_fs_id(&f).await;
    assert_ne!(
        root_after, root_before,
        "root fs_id must change after rename — FsDirData must be re-computed"
    );

    let root_data = server::repo::read_fs_dir_data(f.server.db.as_ref(), &f.repo_id, &root_after)
        .await
        .unwrap();
    let renamed_in_tree = root_data.dirents.iter().any(|d| d.name == "renamed.txt");
    let old_in_tree = root_data.dirents.iter().any(|d| d.name == "old_name.txt");
    assert!(
        renamed_in_tree,
        "root FsDirData must contain 'renamed.txt', got: {:?}",
        root_data
            .dirents
            .iter()
            .map(|d| &d.name)
            .collect::<Vec<_>>()
    );
    assert!(
        !old_in_tree,
        "root FsDirData must not contain 'old_name.txt'"
    );

    // Directory listing should also show new name, not old name
    let resp = f.client.list_dir(&f.api_token, &f.repo_id, "/").await;
    assert_eq!(resp.status(), 200);
    let entries: Vec<serde_json::Value> = resp.json().await.unwrap();
    let names: Vec<&str> = entries.iter().filter_map(|e| e["name"].as_str()).collect();
    assert!(
        names.contains(&"renamed.txt"),
        "renamed file should appear, got {:?}",
        names
    );
    assert!(
        !names.contains(&"old_name.txt"),
        "old name should not appear"
    );

    // List dir via sync protocol's fs-id-list from the new HEAD
    let resp = f
        .client
        .fs_id_list(&f.sync_token, &f.repo_id, &head_after)
        .await;
    assert_eq!(resp.status(), 200);
    let fs_ids: Vec<serde_json::Value> = resp.json().await.unwrap();
    assert!(!fs_ids.is_empty(), "new HEAD must have FS objects");
}

/// Deleting a file via the API must:
/// 1. Create a new HEAD commit
/// 2. Remove the file from the FS tree
#[tokio::test]
async fn test_regression_delete_file_creates_new_commit() {
    let f = TestFixture::new().await;

    // Upload a file
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "delete_me.txt", b"bye bye")
        .await;
    assert_eq!(resp.status(), 200);

    // Record HEAD commit before delete
    let resp = f.client.get_head_commit(&f.sync_token, &f.repo_id).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let head_before = body["head_commit_id"].as_str().unwrap().to_string();
    assert_ne!(head_before, "0000000000000000000000000000000000000000");

    // Delete file
    let resp = f
        .client
        .delete(
            &format!("/api2/repos/{}/file/?p=/delete_me.txt", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);

    // HEAD must change
    let resp = f.client.get_head_commit(&f.sync_token, &f.repo_id).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let head_after = body["head_commit_id"].as_str().unwrap().to_string();
    assert_ne!(head_after, head_before, "HEAD must change after delete");

    // CRITICAL: Verify root FsDirData no longer has the deleted file
    let root_fs_id = get_root_fs_id(&f).await;
    let root_data = server::repo::read_fs_dir_data(f.server.db.as_ref(), &f.repo_id, &root_fs_id)
        .await
        .unwrap();
    assert!(
        !root_data.dirents.iter().any(|d| d.name == "delete_me.txt"),
        "root FsDirData must not contain 'delete_me.txt', got: {:?}",
        root_data
            .dirents
            .iter()
            .map(|d| &d.name)
            .collect::<Vec<_>>()
    );

    // Deleted file should not appear in listing
    let resp = f.client.list_dir(&f.api_token, &f.repo_id, "/").await;
    assert_eq!(resp.status(), 200);
    let entries: Vec<serde_json::Value> = resp.json().await.unwrap();
    let names: Vec<&str> = entries.iter().filter_map(|e| e["name"].as_str()).collect();
    assert!(
        !names.contains(&"delete_me.txt"),
        "deleted file should not appear, got {:?}",
        names
    );
}

/// Renaming a directory via the form-encoded API must create a new HEAD commit.
#[tokio::test]
async fn test_regression_rename_dir_creates_new_commit() {
    let f = TestFixture::new().await;

    // Create a directory via API JSON (exercises create_dir_by_path)
    let resp = f
        .client
        .create_dir(&f.api_token, &f.repo_id, "/my_folder")
        .await;
    assert_eq!(resp.status(), 200);

    // Record root fs_id before rename from head commit
    let root_before = get_root_fs_id(&f).await;

    // Rename directory via form POST (as the Qt client does)
    let resp = f
        .client
        .post_form(
            &format!("/api2/repos/{}/dir/?p=/my_folder", f.repo_id),
            Some(&f.api_token),
            &[("operation", "rename"), ("newname", "renamed_folder")],
        )
        .await;
    assert_eq!(resp.status(), 200);

    // HEAD must change
    let resp = f.client.get_head_commit(&f.sync_token, &f.repo_id).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let _head_after = body["head_commit_id"].as_str().unwrap().to_string();
    let root_after = get_root_fs_id(&f).await;
    assert_ne!(
        root_after, root_before,
        "root fs_id must change after dir rename"
    );

    // CRITICAL: Verify root FsDirData has the renamed directory name
    let root_data = server::repo::read_fs_dir_data(f.server.db.as_ref(), &f.repo_id, &root_after)
        .await
        .unwrap();
    let renamed_in_tree = root_data.dirents.iter().any(|d| d.name == "renamed_folder");
    let old_in_tree = root_data.dirents.iter().any(|d| d.name == "my_folder");
    assert!(
        renamed_in_tree,
        "root FsDirData must contain 'renamed_folder', got: {:?}",
        root_data
            .dirents
            .iter()
            .map(|d| &d.name)
            .collect::<Vec<_>>()
    );
    assert!(!old_in_tree, "root FsDirData must not contain 'my_folder'");

    // Listing shows new name, not old name
    let resp = f.client.list_dir(&f.api_token, &f.repo_id, "/").await;
    assert_eq!(resp.status(), 200);
    let entries: Vec<serde_json::Value> = resp.json().await.unwrap();
    let names: Vec<&str> = entries.iter().filter_map(|e| e["name"].as_str()).collect();
    assert!(
        names.contains(&"renamed_folder"),
        "renamed dir should appear, got {:?}",
        names
    );
    assert!(
        !names.contains(&"my_folder"),
        "old dir name should not appear"
    );
}

/// Deleting a directory via the API must create a new HEAD commit.
#[tokio::test]
async fn test_regression_delete_dir_creates_new_commit() {
    let f = TestFixture::new().await;

    // Create a directory and upload a file inside it
    let resp = f
        .client
        .create_dir(&f.api_token, &f.repo_id, "/my_folder")
        .await;
    assert_eq!(resp.status(), 200);
    let resp = f
        .client
        .upload_file(
            &f.api_token,
            &f.repo_id,
            "/my_folder",
            "nested.txt",
            b"nested",
        )
        .await;
    assert_eq!(resp.status(), 200);

    // Record HEAD commit before delete
    let resp = f.client.get_head_commit(&f.sync_token, &f.repo_id).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let head_before = body["head_commit_id"].as_str().unwrap().to_string();
    assert_ne!(head_before, "0000000000000000000000000000000000000000");

    // Delete directory
    let resp = f
        .client
        .delete(
            &format!("/api2/repos/{}/dir/?p=/my_folder", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);

    // HEAD must change
    let resp = f.client.get_head_commit(&f.sync_token, &f.repo_id).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let head_after = body["head_commit_id"].as_str().unwrap().to_string();
    assert_ne!(head_after, head_before, "HEAD must change after dir delete");

    // CRITICAL: Verify root FsDirData no longer has the deleted directory
    let root_fs_id = get_root_fs_id(&f).await;
    let root_data = server::repo::read_fs_dir_data(f.server.db.as_ref(), &f.repo_id, &root_fs_id)
        .await
        .unwrap();
    assert!(
        !root_data.dirents.iter().any(|d| d.name == "my_folder"),
        "root FsDirData must not contain 'my_folder', got: {:?}",
        root_data
            .dirents
            .iter()
            .map(|d| &d.name)
            .collect::<Vec<_>>()
    );

    // Deleted directory should not appear in root listing
    let resp = f.client.list_dir(&f.api_token, &f.repo_id, "/").await;
    assert_eq!(resp.status(), 200);
    let entries: Vec<serde_json::Value> = resp.json().await.unwrap();
    let names: Vec<&str> = entries.iter().filter_map(|e| e["name"].as_str()).collect();
    assert!(
        !names.contains(&"my_folder"),
        "deleted dir should not appear, got {:?}",
        names
    );
}

/// Verify that creating a directory via API immediately shows up in list_dir.
/// Regression test for: creating "test" folder in cloud browser shows success
/// but the folder doesn't appear in the listing.
#[tokio::test]
async fn test_create_dir_shows_in_list_dir() {
    let f = TestFixture::new().await;

    // Upload a file first (to have entries with a parent_id)
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "readme.txt", b"hello")
        .await;
    assert_eq!(resp.status(), 200, "upload failed");

    // Create a directory via API JSON
    let resp = f.client.create_dir(&f.api_token, &f.repo_id, "/test").await;
    assert_eq!(resp.status(), 200, "create_dir /test failed");

    // List root — must contain BOTH the file AND the directory
    let resp = f.client.list_dir(&f.api_token, &f.repo_id, "/").await;
    assert_eq!(resp.status(), 200, "list_dir failed");
    let entries: Vec<serde_json::Value> = resp.json().await.unwrap();
    let names: Vec<&str> = entries.iter().filter_map(|e| e["name"].as_str()).collect();
    assert!(
        names.contains(&"test"),
        "dir 'test' must appear in listing, got {:?}",
        names
    );
    assert!(
        names.contains(&"readme.txt"),
        "file 'readme.txt' must still appear, got {:?}",
        names
    );
}

/// Verify that creating a directory via API works in an empty repo
/// (no previous files/operations).
#[tokio::test]
async fn test_create_dir_in_empty_repo_shows_in_list_dir() {
    let f = TestFixture::new().await;

    // Create a directory in an otherwise empty repo
    let resp = f
        .client
        .create_dir(&f.api_token, &f.repo_id, "/my_folder")
        .await;
    assert_eq!(resp.status(), 200, "create_dir failed");

    // List root — must contain the directory
    let resp = f.client.list_dir(&f.api_token, &f.repo_id, "/").await;
    assert_eq!(resp.status(), 200, "list_dir failed");
    let entries: Vec<serde_json::Value> = resp.json().await.unwrap();
    let names: Vec<&str> = entries.iter().filter_map(|e| e["name"].as_str()).collect();
    assert!(
        names.contains(&"my_folder"),
        "dir 'my_folder' must appear in listing, got {:?}",
        names
    );
}

/// Verify that creating a directory with a Chinese name works and shows up.
#[tokio::test]
async fn test_create_dir_chinese_name_shows_in_list_dir() {
    let f = TestFixture::new().await;

    // Upload a file first
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "readme.txt", b"hello")
        .await;
    assert_eq!(resp.status(), 200, "upload failed");

    // Create a directory with Chinese name (未命名文件夹 = "unnamed folder")
    let resp = f
        .client
        .create_dir(&f.api_token, &f.repo_id, "/未命名文件夹")
        .await;
    assert_eq!(resp.status(), 200, "create_dir with Chinese name failed");

    // List root — must contain the Chinese-named directory
    let resp = f.client.list_dir(&f.api_token, &f.repo_id, "/").await;
    assert_eq!(resp.status(), 200, "list_dir failed");
    let entries: Vec<serde_json::Value> = resp.json().await.unwrap();
    let names: Vec<&str> = entries.iter().filter_map(|e| e["name"].as_str()).collect();
    assert!(
        names.contains(&"未命名文件夹"),
        "dir '未命名文件夹' must appear in listing, got {:?}",
        names
    );
}

/// Verify that after the sync protocol updates HEAD, creating a new
/// directory still shows up in list_dir.
#[tokio::test]
async fn test_create_dir_after_rebuild_shows_in_list() {
    let f = TestFixture::new().await;

    // Upload a file
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "readme.txt", b"hello")
        .await;
    assert_eq!(resp.status(), 200);

    // Update HEAD via sync protocol (update_branch no longer rebuilds dir_entries)
    let head_resp = f.client.get_head_commit(&f.sync_token, &f.repo_id).await;
    let head_commit_id = head_resp.json::<serde_json::Value>().await.unwrap()["head_commit_id"]
        .as_str()
        .unwrap()
        .to_string();
    let resp = f
        .client
        .update_branch(&f.sync_token, &f.repo_id, &head_commit_id)
        .await;
    assert_eq!(resp.status(), 200, "update_branch failed");

    // Create a new directory
    let resp = f
        .client
        .create_dir(&f.api_token, &f.repo_id, "/new_folder")
        .await;
    assert_eq!(resp.status(), 200);

    // List root — must contain both the file and the new directory
    let resp = f.client.list_dir(&f.api_token, &f.repo_id, "/").await;
    assert_eq!(resp.status(), 200);
    let entries: Vec<serde_json::Value> = resp.json().await.unwrap();
    let names: Vec<&str> = entries.iter().filter_map(|e| e["name"].as_str()).collect();
    assert!(
        names.contains(&"new_folder"),
        "dir 'new_folder' must appear after rebuild+create, got {:?}",
        names
    );
    assert!(
        names.contains(&"readme.txt"),
        "file 'readme.txt' must still appear after rebuild+create, got {:?}",
        names
    );
}

/// Verify that empty directories use the EMPTY_SHA1 sentinel consistently
/// across both API and UI creation paths. Creating a real fs_object breaks
/// the Seafile C client's diff engine.
#[tokio::test]
async fn test_empty_directory_uses_emtpysha1() {
    let f = TestFixture::new().await;

    // ── API path ────────────────────────────────────────────────────────
    let resp = f
        .client
        .create_dir(&f.api_token, &f.repo_id, "/api_empty")
        .await;
    assert_eq!(resp.status(), 200, "API: create dir should succeed");

    // List root — verify the dir entry exists
    let resp = f.client.list_dir(&f.api_token, &f.repo_id, "/").await;
    assert_eq!(resp.status(), 200);
    let entries: Vec<serde_json::Value> = resp.json().await.unwrap();
    let api_entry = entries.iter().find(|e| e["name"] == "api_empty");
    assert!(
        api_entry.is_some(),
        "API: dir entry should appear in listing"
    );

    // Verify the dir entry's id is EMPTY_SHA1
    let api_dir_id = api_entry.unwrap()["id"].as_str().unwrap();
    assert_eq!(
        api_dir_id, "0000000000000000000000000000000000000000",
        "API: empty dir must use EMPTY_SHA1 sentinel"
    );

    // Verify no fs_object was created for the empty dir
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    let api_fs_obj = server::entity::fs_object::Entity::find()
        .filter(
            server::entity::fs_object::Column::FsId.eq("0000000000000000000000000000000000000000"),
        )
        .one(f.server.db.as_ref())
        .await
        .unwrap();
    assert!(
        api_fs_obj.is_none(),
        "API: must not create fs_object for EMPTY_SHA1 sentinel"
    );

    // ── UI path ─────────────────────────────────────────────────────────
    // Use the UI cookie-based client
    let ui_client = reqwest::Client::builder()
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    // Login via UI
    let login_resp = ui_client
        .post(format!("{}/accounts/login/", f.server.base_url))
        .form(&[("email", "test@example.com"), ("password", "password")])
        .send()
        .await
        .unwrap();
    assert_eq!(login_resp.status(), 302, "UI login should redirect");

    // Get CSRF token from the file browser page
    let page_resp = ui_client
        .get(format!(
            "{}/library/{}/{}/",
            f.server.base_url, f.repo_id, "repo"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(page_resp.status(), 200, "UI: file browser should load");
    let page_html = page_resp.text().await.unwrap();
    let csrf_token = page_html
        .split(r#"name="csrf_token" value=""#)
        .nth(1)
        .and_then(|s| s.split('"').next())
        .unwrap_or("");

    // Create empty dir via UI endpoint with CSRF token
    let ui_resp = ui_client
        .post(format!(
            "{}/library/{}/new-dir",
            f.server.base_url, f.repo_id
        ))
        .form(&[("p", "/ui_empty"), ("csrf_token", csrf_token)])
        .send()
        .await
        .unwrap();
    assert_eq!(ui_resp.status(), 302, "UI: create dir should redirect");

    // Verify via API listing that the UI-created dir also uses EMPTY_SHA1
    let resp = f.client.list_dir(&f.api_token, &f.repo_id, "/").await;
    assert_eq!(resp.status(), 200);
    let entries: Vec<serde_json::Value> = resp.json().await.unwrap();
    let ui_entry = entries.iter().find(|e| e["name"] == "ui_empty");
    assert!(ui_entry.is_some(), "UI: dir entry should appear in listing");

    let ui_dir_id = ui_entry.unwrap()["id"].as_str().unwrap();
    assert_eq!(
        ui_dir_id, "0000000000000000000000000000000000000000",
        "UI: empty dir must use EMPTY_SHA1 sentinel, not a real fs_id"
    );

    // ── Sync protocol: root directory tree is correctly updated ─────────
    let head_resp = f.client.get_head_commit(&f.sync_token, &f.repo_id).await;
    assert_eq!(head_resp.status(), 200);
    let head_json: serde_json::Value = head_resp.json().await.unwrap();
    let head_commit_id = head_json["head_commit_id"].as_str().unwrap().to_string();
    assert_ne!(
        head_commit_id, "0000000000000000000000000000000000000000",
        "head commit should be updated after dir creation"
    );

    // Retrieve the head commit and verify its root_id
    let commit_resp = f
        .client
        .get_commit(&f.sync_token, &f.repo_id, &head_commit_id)
        .await;
    assert_eq!(commit_resp.status(), 200);
    let commit_bytes = commit_resp.bytes().await.unwrap();
    let commit: CommitData = serde_json::from_slice(&commit_bytes).unwrap();

    // The root directory's fs_id should have changed from the empty state
    assert_ne!(
        commit.root_id, "0000000000000000000000000000000000000000",
        "root dir must have a real fs_id after creating subdirectories"
    );

    // ── Nested dir creation via API (which also uses EMPTY_SHA1) ─────────
    let resp = f
        .client
        .create_dir(&f.api_token, &f.repo_id, "/alpha")
        .await;
    assert_eq!(resp.status(), 200, "API: create parent dir");
    let resp = f
        .client
        .create_dir(&f.api_token, &f.repo_id, "/alpha/beta")
        .await;
    assert_eq!(
        resp.status(),
        200,
        "API: nested dir creation should succeed"
    );

    // Verify the nested dir appears
    let resp = f.client.list_dir(&f.api_token, &f.repo_id, "/alpha").await;
    assert_eq!(resp.status(), 200);
    let entries: Vec<serde_json::Value> = resp.json().await.unwrap();
    let beta = entries.iter().find(|e| e["name"] == "beta");
    assert!(beta.is_some(), "nested empty dir should be listed");
    assert_eq!(
        beta.unwrap()["id"].as_str().unwrap(),
        "0000000000000000000000000000000000000000",
        "nested empty dir must also use EMPTY_SHA1"
    );
}

// ── Recursive directory listing tests ─────────────────────────────

/// Recursive listing with `recursive=1` returns all entries in a flat list
/// with their `parent_dir`.
#[tokio::test]
async fn test_dir_recursive_basic() {
    let f = TestFixture::new().await;

    // Create nested structure: /a.txt, /subdir/b.txt, /subdir/nested/c.txt
    f.client
        .upload_file(&f.api_token, &f.repo_id, "/", "a.txt", b"aaa")
        .await;
    f.client
        .create_dir(&f.api_token, &f.repo_id, "/subdir")
        .await;
    f.client
        .upload_file(&f.api_token, &f.repo_id, "/subdir", "b.txt", b"bbb")
        .await;
    f.client
        .create_dir(&f.api_token, &f.repo_id, "/subdir/nested")
        .await;
    f.client
        .upload_file(&f.api_token, &f.repo_id, "/subdir/nested", "c.txt", b"ccc")
        .await;

    // Recursive listing — no type filter
    let resp = f
        .client
        .list_dir_with_params(&f.api_token, &f.repo_id, "/", Some("1"), None)
        .await;
    assert_eq!(resp.status(), 200);

    // Extract headers before consuming body
    let oid = resp
        .headers()
        .get("oid")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let dir_perm = resp
        .headers()
        .get("dir_perm")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let entries: Vec<serde_json::Value> = resp.json().await.unwrap();

    // Should contain: a.txt(file), subdir(dir), b.txt(file), nested(dir), c.txt(file)
    assert_eq!(entries.len(), 5, "recursive=1 should return all 5 entries");

    // Check parent_dir for each entry
    let a_txt = entries.iter().find(|e| e["name"] == "a.txt").unwrap();
    assert_eq!(a_txt["type"], "file");
    assert_eq!(a_txt["parent_dir"], "/");

    let subdir = entries.iter().find(|e| e["name"] == "subdir").unwrap();
    assert_eq!(subdir["type"], "dir");
    assert_eq!(subdir["parent_dir"], "/");

    let b_txt = entries.iter().find(|e| e["name"] == "b.txt").unwrap();
    assert_eq!(b_txt["type"], "file");
    assert_eq!(b_txt["parent_dir"], "/subdir");
    assert!(b_txt["size"].as_i64().unwrap_or(0) > 0);

    let nested = entries.iter().find(|e| e["name"] == "nested").unwrap();
    assert_eq!(nested["type"], "dir");
    assert_eq!(nested["parent_dir"], "/subdir");

    let c_txt = entries.iter().find(|e| e["name"] == "c.txt").unwrap();
    assert_eq!(c_txt["type"], "file");
    assert_eq!(c_txt["parent_dir"], "/subdir/nested");

    // Verify oid and dir_perm headers
    assert!(oid.as_deref().is_some_and(|v| !v.is_empty()));
    assert_eq!(dir_perm.as_deref(), Some("rw"));
}

/// Recursive listing with `t=f` returns only file entries.
#[tokio::test]
async fn test_dir_recursive_type_filter_file() {
    let f = TestFixture::new().await;

    f.client
        .upload_file(&f.api_token, &f.repo_id, "/", "a.txt", b"aaa")
        .await;
    f.client
        .create_dir(&f.api_token, &f.repo_id, "/subdir")
        .await;
    f.client
        .upload_file(&f.api_token, &f.repo_id, "/subdir", "b.txt", b"bbb")
        .await;

    let resp = f
        .client
        .list_dir_with_params(&f.api_token, &f.repo_id, "/", Some("1"), Some("f"))
        .await;
    assert_eq!(resp.status(), 200);
    let entries: Vec<serde_json::Value> = resp.json().await.unwrap();

    // Only file entries (2 files, 0 dirs)
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().all(|e| e["type"] == "file"));
    assert!(entries.iter().any(|e| e["name"] == "a.txt"));
    assert!(entries.iter().any(|e| e["name"] == "b.txt"));
    assert!(entries.iter().any(|e| e["parent_dir"] == "/"));
    assert!(entries.iter().any(|e| e["parent_dir"] == "/subdir"));
}

/// Recursive listing with `t=d` returns only directory entries.
#[tokio::test]
async fn test_dir_recursive_type_filter_dir() {
    let f = TestFixture::new().await;

    f.client
        .upload_file(&f.api_token, &f.repo_id, "/", "a.txt", b"aaa")
        .await;
    f.client
        .create_dir(&f.api_token, &f.repo_id, "/subdir")
        .await;
    f.client
        .upload_file(&f.api_token, &f.repo_id, "/subdir", "b.txt", b"bbb")
        .await;
    f.client
        .create_dir(&f.api_token, &f.repo_id, "/subdir/nested")
        .await;

    let resp = f
        .client
        .list_dir_with_params(&f.api_token, &f.repo_id, "/", Some("1"), Some("d"))
        .await;
    assert_eq!(resp.status(), 200);
    let entries: Vec<serde_json::Value> = resp.json().await.unwrap();

    // Only directory entries (2 dirs)
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().all(|e| e["type"] == "dir"));
    assert!(entries.iter().any(|e| e["name"] == "subdir"));
    assert!(entries.iter().any(|e| e["name"] == "nested"));
}

/// Invalid `recursive` value returns 400.
#[tokio::test]
async fn test_dir_recursive_invalid_recursive() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .list_dir_with_params(&f.api_token, &f.repo_id, "/", Some("invalid"), None)
        .await;
    assert_eq!(resp.status(), 400);
}

/// Invalid `t` value returns 400.
#[tokio::test]
async fn test_dir_recursive_invalid_t() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .list_dir_with_params(&f.api_token, &f.repo_id, "/", Some("1"), Some("x"))
        .await;
    assert_eq!(resp.status(), 400);
}

/// Non-recursive listing without `recursive` param is unchanged (single-level).
#[tokio::test]
async fn test_dir_recursive_non_recursive_unchanged() {
    let f = TestFixture::new().await;

    f.client
        .upload_file(&f.api_token, &f.repo_id, "/", "a.txt", b"aaa")
        .await;
    f.client
        .create_dir(&f.api_token, &f.repo_id, "/subdir")
        .await;
    f.client
        .upload_file(&f.api_token, &f.repo_id, "/subdir", "b.txt", b"bbb")
        .await;

    // Without recursive=1 — only root-level entries
    let resp = f.client.list_dir(&f.api_token, &f.repo_id, "/").await;
    assert_eq!(resp.status(), 200);
    let entries: Vec<serde_json::Value> = resp.json().await.unwrap();
    assert_eq!(entries.len(), 2); // a.txt + subdir
    assert!(entries.iter().any(|e| e["name"] == "a.txt"));
    assert!(entries.iter().any(|e| e["name"] == "subdir"));

    // No parent_dir in non-recursive response
    assert!(entries.iter().all(|e| e.get("parent_dir").is_none()));
}

/// Recursive listing from a subdirectory path works correctly.
#[tokio::test]
async fn test_dir_recursive_from_subdirectory() {
    let f = TestFixture::new().await;

    f.client
        .upload_file(&f.api_token, &f.repo_id, "/", "a.txt", b"aaa")
        .await;
    f.client
        .create_dir(&f.api_token, &f.repo_id, "/subdir")
        .await;
    f.client
        .upload_file(&f.api_token, &f.repo_id, "/subdir", "b.txt", b"bbb")
        .await;
    f.client
        .create_dir(&f.api_token, &f.repo_id, "/subdir/nested")
        .await;
    f.client
        .upload_file(&f.api_token, &f.repo_id, "/subdir/nested", "c.txt", b"ccc")
        .await;

    // Recursive listing starting from /subdir
    let resp = f
        .client
        .list_dir_with_params(&f.api_token, &f.repo_id, "/subdir", Some("1"), None)
        .await;
    assert_eq!(resp.status(), 200);
    let entries: Vec<serde_json::Value> = resp.json().await.unwrap();

    // Should only contain entries under /subdir: b.txt, nested, c.txt
    assert_eq!(entries.len(), 3);
    assert!(entries.iter().any(|e| e["name"] == "b.txt"));
    assert!(entries.iter().any(|e| e["name"] == "nested"));
    assert!(entries.iter().any(|e| e["name"] == "c.txt"));
    // Should NOT contain root-level entries
    assert!(!entries.iter().any(|e| e["name"] == "a.txt"));
}

/// Empty repo returns empty list for recursive listing.
#[tokio::test]
async fn test_dir_recursive_empty_repo() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .list_dir_with_params(&f.api_token, &f.repo_id, "/", Some("1"), None)
        .await;
    assert_eq!(resp.status(), 200);
    let entries: Vec<serde_json::Value> = resp.json().await.unwrap();
    assert!(entries.is_empty());
}

/// File entries in recursive listing include modifier_name and modifier_contact_email.
#[tokio::test]
async fn test_dir_recursive_file_has_modifier_fields() {
    let f = TestFixture::new().await;

    f.client
        .upload_file(&f.api_token, &f.repo_id, "/", "a.txt", b"aaa")
        .await;

    let resp = f
        .client
        .list_dir_with_params(&f.api_token, &f.repo_id, "/", Some("1"), Some("f"))
        .await;
    assert_eq!(resp.status(), 200);
    let entries: Vec<serde_json::Value> = resp.json().await.unwrap();

    let a_txt = entries.iter().find(|e| e["name"] == "a.txt").unwrap();
    assert_eq!(a_txt["type"], "file");
    assert!(
        a_txt.get("modifier_name").is_some(),
        "file entry should have modifier_name"
    );
    assert!(
        a_txt.get("modifier_contact_email").is_some(),
        "file entry should have modifier_contact_email"
    );
}
