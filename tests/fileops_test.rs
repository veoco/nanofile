mod common;

use common::TestFixture;

/// Helper: upload a file to a repo and return success.
async fn upload_test_file(f: &TestFixture, name: &str, data: &[u8], parent_dir: &str) {
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, parent_dir, name, data)
        .await;
    assert_eq!(
        resp.status(),
        200,
        "upload {} failed: {:?}",
        name,
        resp.text().await
    );
}

/// Helper: create a subdirectory.
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

/// Helper: list directory contents and return count of entries.
async fn dir_entry_count(f: &TestFixture, path: &str) -> usize {
    let resp = f.client.list_dir(&f.api_token, &f.repo_id, path).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    body.as_array().map(|a| a.len()).unwrap_or_else(|| {
        // v2.1 format wraps in object
        body.get("dirent_list")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0)
    })
}

// ======================================================================
// Batch Delete Tests
// ======================================================================

#[tokio::test]
async fn test_batch_delete_single_file() {
    let f = TestFixture::new().await;

    // Upload two files
    upload_test_file(&f, "file1.txt", b"content1", "/").await;
    upload_test_file(&f, "file2.txt", b"content2", "/").await;

    // Verify both files exist
    assert!(
        dir_entry_count(&f, "/").await >= 2,
        "expected at least 2 entries in root"
    );

    // Batch delete one file
    let resp = f
        .client
        .batch_delete(&f.api_token, &f.repo_id, "/", &["file1.txt"])
        .await;
    assert_eq!(
        resp.status(),
        200,
        "batch delete failed: {:?}",
        resp.text().await
    );

    // Verify only file2 remains
    let resp = f.client.list_dir(&f.api_token, &f.repo_id, "/").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let names: Vec<&str> = body
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v["name"].as_str())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    assert!(!names.contains(&"file1.txt"), "file1 should be deleted");
    assert!(names.contains(&"file2.txt"), "file2 should remain");
}

#[tokio::test]
async fn test_batch_delete_multiple_files() {
    let f = TestFixture::new().await;

    upload_test_file(&f, "a.txt", b"aaa", "/").await;
    upload_test_file(&f, "b.txt", b"bbb", "/").await;
    upload_test_file(&f, "c.txt", b"ccc", "/").await;

    // Batch delete two files
    let resp = f
        .client
        .batch_delete(&f.api_token, &f.repo_id, "/", &["a.txt", "b.txt"])
        .await;
    assert_eq!(resp.status(), 200);

    let resp = f.client.list_dir(&f.api_token, &f.repo_id, "/").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let names: Vec<&str> = body
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v["name"].as_str())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    assert!(!names.contains(&"a.txt"));
    assert!(!names.contains(&"b.txt"));
    assert!(names.contains(&"c.txt"));
}

#[tokio::test]
async fn test_batch_delete_empty_list() {
    let f = TestFixture::new().await;

    upload_test_file(&f, "keep.txt", b"keep", "/").await;

    // Empty file_names should return success without error
    let resp = f
        .client
        .batch_delete(&f.api_token, &f.repo_id, "/", &[])
        .await;
    assert_eq!(resp.status(), 200);

    // File should still exist
    let resp = f.client.list_dir(&f.api_token, &f.repo_id, "/").await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_batch_delete_reloaddir() {
    let f = TestFixture::new().await;

    upload_test_file(&f, "del1.txt", b"del1", "/").await;
    upload_test_file(&f, "del2.txt", b"del2", "/").await;

    // Delete with reloaddir=true
    let resp = f
        .client
        .batch_delete_with_dir(&f.api_token, &f.repo_id, "/", &["del1.txt", "del2.txt"])
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    // Should have dir_listing field with remaining entries
    assert!(
        body.get("dir_listing").is_some(),
        "reloaddir=true should return dir_listing"
    );
}

#[tokio::test]
async fn test_batch_delete_no_auth() {
    let f = TestFixture::new().await;

    upload_test_file(&f, "secret.txt", b"secret", "/").await;

    let resp = f
        .client
        .batch_delete("invalid-token", &f.repo_id, "/", &["secret.txt"])
        .await;
    // Should fail with 401 or 403
    assert!(
        resp.status() == 401 || resp.status() == 403,
        "expected 401 or 403, got {}",
        resp.status()
    );
}

// ======================================================================
// Batch Copy Tests
// ======================================================================

#[tokio::test]
async fn test_batch_copy_same_repo() {
    let f = TestFixture::new().await;

    upload_test_file(&f, "src.txt", b"source content", "/").await;
    create_subdir(&f, "/dest").await;

    let resp = f
        .client
        .batch_copy(
            &f.api_token,
            &f.repo_id,
            "/",
            &["src.txt"],
            &f.repo_id,
            "/dest",
        )
        .await;
    assert_eq!(
        resp.status(),
        200,
        "batch copy failed: {:?}",
        resp.text().await
    );

    // Verify response format
    let body: serde_json::Value = resp.json().await.unwrap();
    let results = body.as_array().expect("response should be a JSON array");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["repo_id"].as_str(), Some(f.repo_id.as_str()));
    assert_eq!(results[0]["parent_dir"].as_str(), Some("/dest"));
    assert_eq!(results[0]["obj_name"].as_str(), Some("src.txt"));

    // Verify file exists at destination
    let resp = f.client.list_dir(&f.api_token, &f.repo_id, "/dest").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let names: Vec<&str> = body
        .as_array()
        .map(|a| a.iter().filter_map(|v| v["name"].as_str()).collect())
        .unwrap_or_default();
    assert!(
        names.contains(&"src.txt"),
        "copied file should exist in /dest"
    );
}

#[tokio::test]
async fn test_batch_copy_name_collision() {
    let f = TestFixture::new().await;

    // Upload the same filename to root and dest
    upload_test_file(&f, "collide.txt", b"original", "/").await;
    create_subdir(&f, "/target").await;
    upload_test_file(&f, "collide.txt", b"original", "/target").await;

    // Copy — destination already has collide.txt
    let resp = f
        .client
        .batch_copy(
            &f.api_token,
            &f.repo_id,
            "/",
            &["collide.txt"],
            &f.repo_id,
            "/target",
        )
        .await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    // Should have auto-renamed name (e.g. "collide (1).txt")
    let obj_name = body[0]["obj_name"].as_str().unwrap();
    assert_ne!(obj_name, "collide.txt", "should auto-rename on collision");
    assert!(
        obj_name.contains("collide"),
        "name should contain original stem"
    );
}

#[tokio::test]
async fn test_batch_copy_cross_repo_fails() {
    let f = TestFixture::new().await;

    upload_test_file(&f, "xfer.txt", b"data", "/").await;

    // Try cross-repo copy with a fake repo ID
    let resp = f
        .client
        .batch_copy(
            &f.api_token,
            &f.repo_id,
            "/",
            &["xfer.txt"],
            "nonexistent-repo-id",
            "/",
        )
        .await;
    // Should fail with 400 (cross-repo not supported)
    assert_eq!(resp.status(), 400);
}

// ======================================================================
// Batch Move Tests
// ======================================================================

#[tokio::test]
async fn test_batch_move_same_repo() {
    let f = TestFixture::new().await;

    upload_test_file(&f, "moveme.txt", b"move content", "/").await;
    create_subdir(&f, "/sub").await;

    let resp = f
        .client
        .batch_move(
            &f.api_token,
            &f.repo_id,
            "/",
            &["moveme.txt"],
            &f.repo_id,
            "/sub",
        )
        .await;
    assert_eq!(
        resp.status(),
        200,
        "batch move failed: {:?}",
        resp.text().await
    );

    // Verify response format
    let body: serde_json::Value = resp.json().await.unwrap();
    let results = body.as_array().expect("response should be a JSON array");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["repo_id"].as_str(), Some(f.repo_id.as_str()));
    assert_eq!(results[0]["parent_dir"].as_str(), Some("/sub"));
    assert_eq!(results[0]["obj_name"].as_str(), Some("moveme.txt"));

    // File should be at destination, not source
    let resp = f.client.list_dir(&f.api_token, &f.repo_id, "/sub").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let sub_names: Vec<&str> = body
        .as_array()
        .map(|a| a.iter().filter_map(|v| v["name"].as_str()).collect())
        .unwrap_or_default();
    assert!(sub_names.contains(&"moveme.txt"));

    let resp = f.client.list_dir(&f.api_token, &f.repo_id, "/").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let root_names: Vec<&str> = body
        .as_array()
        .map(|a| a.iter().filter_map(|v| v["name"].as_str()).collect())
        .unwrap_or_default();
    assert!(!root_names.contains(&"moveme.txt"));
}

#[tokio::test]
async fn test_batch_move_name_collision() {
    let f = TestFixture::new().await;

    upload_test_file(&f, "conflict.txt", b"source", "/").await;
    create_subdir(&f, "/dest").await;
    upload_test_file(&f, "conflict.txt", b"destination", "/dest").await;

    let resp = f
        .client
        .batch_move(
            &f.api_token,
            &f.repo_id,
            "/",
            &["conflict.txt"],
            &f.repo_id,
            "/dest",
        )
        .await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let obj_name = body[0]["obj_name"].as_str().unwrap();
    assert_ne!(obj_name, "conflict.txt", "should auto-rename on collision");
}

#[tokio::test]
async fn test_batch_move_cross_repo_fails() {
    let f = TestFixture::new().await;

    upload_test_file(&f, "mymove.txt", b"data", "/").await;

    let resp = f
        .client
        .batch_move(
            &f.api_token,
            &f.repo_id,
            "/",
            &["mymove.txt"],
            "bad-repo",
            "/",
        )
        .await;
    assert_eq!(resp.status(), 400);
}
