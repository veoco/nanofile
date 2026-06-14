mod common;

use common::TestFixture;

// ── Helpers ──────────────────────────────────────────────────────────────

/// Upload a small file to a repo.
async fn upload_file(f: &TestFixture, name: &str) {
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", name, b"content")
        .await;
    assert_eq!(
        resp.status(),
        200,
        "upload {} failed: {:?}",
        name,
        resp.text().await
    );
}

/// Create a subdirectory.
async fn create_subdir(f: &TestFixture, path: &str) {
    let resp = f.client.create_dir(&f.api_token, &f.repo_id, path).await;
    assert_eq!(
        resp.status(),
        200,
        "mkdir {} failed: {:?}",
        path,
        resp.text().await
    );
}

// ── Existing tests ───────────────────────────────────────────────────────

#[tokio::test]
async fn test_starred_files_empty() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .get("/api2/starredfiles/", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_starred_files_unauthorized() {
    let f = TestFixture::new().await;

    let resp = f.client.get("/api2/starredfiles/", None).await;
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_v21_starred_items_star_and_unstar() {
    let f = TestFixture::new().await;

    // Upload a file first
    upload_file(&f, "star-me.txt").await;

    // Star it via v2.1
    let resp = f
        .client
        .post_json(
            "/api/v2.1/starred-items/",
            Some(&f.api_token),
            &serde_json::json!({"repo_id": f.repo_id, "path": "/star-me.txt"}),
        )
        .await;
    assert_eq!(resp.status(), 200, "star failed: {:?}", resp.text().await);

    // List starred via v2.1
    let resp = f
        .client
        .get("/api/v2.1/starred-items/", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let items = body["starred_item_list"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["repo_id"], f.repo_id);
    assert_eq!(items[0]["path"], "/star-me.txt");

    // Legacy API should also show it
    let resp = f
        .client
        .get("/api2/starredfiles/", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);
    let legacy: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(legacy.as_array().unwrap().len(), 1);

    // Unstar via v2.1
    let resp = f
        .client
        .unstar_item(&f.api_token, &f.repo_id, "/star-me.txt")
        .await;
    assert_eq!(resp.status(), 200);

    // List should be empty now
    let resp = f.client.list_starred(&f.api_token).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["starred_item_list"].as_array().unwrap().len(), 0);
}

// ── New star feature tests ───────────────────────────────────────────────

#[tokio::test]
async fn test_star_directory() {
    let f = TestFixture::new().await;
    create_subdir(&f, "/subdir").await;

    // Star the directory
    let resp = f
        .client
        .star_item(&f.api_token, &f.repo_id, "/subdir/")
        .await;
    assert_eq!(
        resp.status(),
        200,
        "star dir failed: {:?}",
        resp.text().await
    );

    // Verify in GET it's classified as starred_folders with is_dir=true
    let body: serde_json::Value = f
        .client
        .list_starred(&f.api_token)
        .await
        .json()
        .await
        .unwrap();
    let items = body["starred_item_list"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["is_dir"], true);
    assert!(items[0]["path"].as_str().unwrap().ends_with('/'));
    assert_eq!(items[0]["obj_name"], "subdir");
}

#[tokio::test]
async fn test_star_root_repo() {
    let f = TestFixture::new().await;

    // Star the repo root
    let resp = f.client.star_item(&f.api_token, &f.repo_id, "/").await;
    assert_eq!(
        resp.status(),
        200,
        "star root failed: {:?}",
        resp.text().await
    );

    // Verify it's classified as starred_repos (path=="/")
    let body: serde_json::Value = f
        .client
        .list_starred(&f.api_token)
        .await
        .json()
        .await
        .unwrap();
    let items = body["starred_item_list"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    let item = &items[0];
    assert_eq!(item["path"], "/");
    assert_eq!(item["is_dir"], true);
    // For a repo root, obj_name should be the repo name
    assert_eq!(item["obj_name"], item["repo_name"]);
}

#[tokio::test]
async fn test_star_nonexistent_path() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .star_item(&f.api_token, &f.repo_id, "/nonexistent.txt")
        .await;
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn test_star_nonexistent_repo() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .star_item(
            &f.api_token,
            "0000000000000000000000000000000000000000",
            "/file.txt",
        )
        .await;
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn test_star_duplicate() {
    let f = TestFixture::new().await;
    upload_file(&f, "dup.txt").await;

    // Star once
    let resp = f
        .client
        .star_item(&f.api_token, &f.repo_id, "/dup.txt")
        .await;
    assert_eq!(resp.status(), 200);

    // Star again — should return 200 (not error) with existing item info
    let resp = f
        .client
        .star_item(&f.api_token, &f.repo_id, "/dup.txt")
        .await;
    assert_eq!(resp.status(), 200);

    // Should only appear once in the list
    let body: serde_json::Value = f
        .client
        .list_starred(&f.api_token)
        .await
        .json()
        .await
        .unwrap();
    let items = body["starred_item_list"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["path"], "/dup.txt");
}

#[tokio::test]
async fn test_unstar_nonexistent() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .unstar_item(&f.api_token, &f.repo_id, "/no-such-file.txt")
        .await;
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn test_star_unauthorized() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .post_json(
            "/api/v2.1/starred-items/",
            None,
            &serde_json::json!({"repo_id": f.repo_id, "path": "/file.txt"}),
        )
        .await;
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_star_multipart() {
    let f = TestFixture::new().await;
    upload_file(&f, "multi.txt").await;

    // Star via multipart/form-data (Android client format)
    let form = reqwest::multipart::Form::new()
        .text("repo_id", f.repo_id.clone())
        .text("path", "/multi.txt");
    let resp = f
        .client
        .post_multipart("/api/v2.1/starred-items/", Some(&f.api_token), form)
        .await;
    assert_eq!(
        resp.status(),
        200,
        "multipart star failed: {:?}",
        resp.text().await
    );
}

#[tokio::test]
async fn test_star_file_in_subdir() {
    let f = TestFixture::new().await;
    create_subdir(&f, "/sub").await;

    // Create empty file in subdirectory via v2.1 endpoint
    let resp = f
        .client
        .post_json(
            &format!("/api/v2.1/repos/{}/file/", f.repo_id),
            Some(&f.api_token),
            &serde_json::json!({"p": "/sub/deep.txt"}),
        )
        .await;
    assert_eq!(resp.status(), 200);

    // Star the file in the subdirectory
    let resp = f
        .client
        .star_item(&f.api_token, &f.repo_id, "/sub/deep.txt")
        .await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = f
        .client
        .list_starred(&f.api_token)
        .await
        .json()
        .await
        .unwrap();
    let items = body["starred_item_list"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["path"], "/sub/deep.txt");
    assert_eq!(items[0]["obj_name"], "deep.txt");
}

#[tokio::test]
async fn test_star_response_fields() {
    let f = TestFixture::new().await;
    upload_file(&f, "field-test.txt").await;

    let resp = f
        .client
        .star_item(&f.api_token, &f.repo_id, "/field-test.txt")
        .await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = f
        .client
        .list_starred(&f.api_token)
        .await
        .json()
        .await
        .unwrap();
    let item = &body["starred_item_list"].as_array().unwrap()[0];

    // Verify all expected response fields
    assert_eq!(item["repo_id"], f.repo_id);
    assert!(!item["repo_name"].as_str().unwrap_or("").is_empty());
    assert_eq!(item["path"], "/field-test.txt");
    assert_eq!(item["obj_name"], "field-test.txt");
    assert_eq!(item["is_dir"], false);
    assert!(
        !item["mtime"].as_str().unwrap_or("").is_empty(),
        "mtime should be a non-empty ISO string"
    );
    assert_eq!(item["deleted"], false);
    assert_eq!(item["user_email"], f.email);
    assert!(!item["user_name"].as_str().unwrap_or("").is_empty());
    assert_eq!(item["repo_encrypted"], false);
}

#[tokio::test]
async fn test_star_deleted_repo() {
    let f = TestFixture::new().await;
    upload_file(&f, "gone.txt").await;
    f.client
        .star_item(&f.api_token, &f.repo_id, "/gone.txt")
        .await;

    // Verify star exists before delete
    let body: serde_json::Value = f
        .client
        .list_starred(&f.api_token)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(body["starred_item_list"].as_array().unwrap().len(), 1);

    // Delete the repo — FK CASCADE removes the star records
    let resp = f.client.delete_repo(&f.api_token, &f.repo_id).await;
    assert_eq!(
        resp.status(),
        200,
        "delete repo failed: {:?}",
        resp.text().await
    );

    // Starred list should be empty (CASCADE delete on FK)
    let body: serde_json::Value = f
        .client
        .list_starred(&f.api_token)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(body["starred_item_list"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_star_legacy_list_after_star() {
    let f = TestFixture::new().await;
    upload_file(&f, "legacy.txt").await;

    f.client
        .star_item(&f.api_token, &f.repo_id, "/legacy.txt")
        .await;

    // Legacy API should return it with is_dir field
    let resp = f
        .client
        .get("/api2/starredfiles/", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);
    let legacy: serde_json::Value = resp.json().await.unwrap();
    let items = legacy.as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["repo_id"], f.repo_id);
    assert_eq!(items[0]["is_dir"], false);
}

#[tokio::test]
async fn test_v21_star_sort_order() {
    let f = TestFixture::new().await;
    create_subdir(&f, "/a").await;
    upload_file(&f, "first.txt").await;
    upload_file(&f, "second.txt").await;

    // Star in order: second.txt, /a/, first.txt
    // Use 1-second delays because created_at has second granularity
    f.client
        .star_item(&f.api_token, &f.repo_id, "/second.txt")
        .await;
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    f.client.star_item(&f.api_token, &f.repo_id, "/a/").await;
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    f.client
        .star_item(&f.api_token, &f.repo_id, "/first.txt")
        .await;

    let body: serde_json::Value = f
        .client
        .list_starred(&f.api_token)
        .await
        .json()
        .await
        .unwrap();
    let items = body["starred_item_list"].as_array().unwrap();
    assert_eq!(items.len(), 3);

    // Items are grouped: repos → folders → files. Each group sorted by mtime desc.
    // Our items: "first.txt" (file), "second.txt" (file), "/a/" (dir after uploads).
    // Expected order: ["a/" (folder), "first.txt" or "second.txt" (files)]
    let names: Vec<&str> = items
        .iter()
        .map(|i| i["obj_name"].as_str().unwrap_or(""))
        .collect();
    eprintln!("sort order names: {:?}", names);

    // First item should be the directory (dirs come before files in grouped output)
    assert_eq!(
        items[0]["obj_name"], "a",
        "expected directory first in grouped output"
    );
    assert_eq!(items[0]["is_dir"], true);
    // Remaining should be files
    assert_eq!(items[1]["is_dir"], false);
    assert_eq!(items[2]["is_dir"], false);
}

#[tokio::test]
async fn test_ui_starred_page() {
    let f = TestFixture::new().await;

    // Use a single cookie client for both login and page requests
    let ui_client = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();
    let login_resp = ui_client
        .post(format!("{}/accounts/login/", f.server.base_url))
        .form(&[("email", f.email.as_str()), ("password", "password")])
        .send()
        .await
        .unwrap();
    assert!(
        login_resp.status() == 302 || login_resp.status() == 200,
        "login failed with status {}: {:?}",
        login_resp.status(),
        login_resp.text().await
    );

    let resp = ui_client
        .get(format!("{}/starred/", f.server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "starred page failed: {:?}",
        resp.text().await
    );
    let html = resp.text().await.unwrap();
    assert!(
        html.contains("Starred"),
        "page should contain 'Starred', got first 300 chars: {:?}",
        &html[..html.len().min(300)]
    );
}

#[tokio::test]
async fn test_ui_unstar_form() {
    let f = TestFixture::new().await;
    upload_file(&f, "ui-unstar.txt").await;
    f.client
        .star_item(&f.api_token, &f.repo_id, "/ui-unstar.txt")
        .await;

    // Use a single cookie client for login and the unstar form
    let ui_client = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();
    let login_resp = ui_client
        .post(format!("{}/accounts/login/", f.server.base_url))
        .form(&[("email", f.email.as_str()), ("password", "password")])
        .send()
        .await
        .unwrap();
    assert!(
        login_resp.status() == 302 || login_resp.status() == 200,
        "login failed with status {}: {:?}",
        login_resp.status(),
        login_resp.text().await
    );

    // POST to /starred/ to unstar via form
    let resp = ui_client
        .post(format!("{}/starred/", f.server.base_url))
        .form(&[("repo_id", f.repo_id.as_str()), ("path", "/ui-unstar.txt")])
        .send()
        .await
        .unwrap();
    // Should return 200 or redirect
    let status = resp.status();
    assert!(
        status == 302 || status == 303 || status == 200,
        "expected redirect status, got: {status}"
    );

    // Verify empty via API
    let body: serde_json::Value = f
        .client
        .list_starred(&f.api_token)
        .await
        .json()
        .await
        .unwrap();
    assert!(body["starred_item_list"].as_array().unwrap().is_empty());
}
