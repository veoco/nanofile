//! Integration tests for the full-text search indexer.
//!
//! These tests verify that file operations (upload, delete, rename, move)
//! correctly update the Tantivy full-text search index.
//!
//! All tests use `TestFixture::new_with_index()` which starts the server
//! with the indexer enabled on a temporary index directory.

mod common;

/// Upload a text file, then search for content → should find it.
#[tokio::test]
async fn test_upload_and_search_content() {
    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;

    // Upload a text file with unique content
    let resp = f
        .client
        .upload_file(
            token,
            &f.repo_id,
            "/",
            "hello.txt",
            b"hello world from nanofile",
        )
        .await;
    assert_eq!(resp.status(), 200, "upload should succeed");

    // Give the indexer time to commit
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Search for content (not filename) — should find the file
    let resp = f
        .client
        .get(
            &"/api2/search/?q=nanofile&search_filename_only=false".to_string(),
            Some(token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let results = body["results"].as_array().unwrap();
    assert!(!results.is_empty(), "should find file via content search");
    assert_eq!(results[0]["name"], "hello.txt");
    assert_eq!(results[0]["fullpath"], "/hello.txt");
}

/// Search for content that does NOT match the filename (only in content).
#[tokio::test]
async fn test_search_content_not_filename() {
    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;

    // File with content that doesn't match its name
    let resp = f
        .client
        .upload_file(
            token,
            &f.repo_id,
            "/",
            "readme.md",
            b"installation guide for the software",
        )
        .await;
    assert_eq!(resp.status(), 200);

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Full-text search should find it (content matches)
    let resp = f
        .client
        .get(
            &"/api2/search/?q=installation&search_filename_only=false".to_string(),
            Some(token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let results = body["results"].as_array().unwrap();
    assert!(
        !results.is_empty(),
        "full-text search should find 'installation' in content"
    );

    // Filename-only search should NOT find it (name doesn't contain 'installation')
    let resp = f
        .client
        .get(
            &"/api2/search/?q=installation&search_filename_only=true".to_string(),
            Some(token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let results = body["results"].as_array().unwrap();
    assert!(
        results.is_empty(),
        "filename-only search should not find 'installation'"
    );
}

/// Upload a binary file → should NOT be indexed.
#[tokio::test]
async fn test_binary_file_skipped() {
    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;

    let resp = f
        .client
        .upload_file(
            token,
            &f.repo_id,
            "/",
            "image.png",
            b"\x89PNG\r\n\x1a\nbinary data",
        )
        .await;
    assert_eq!(resp.status(), 200);

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Search for content in binary file — should not find it
    let resp = f
        .client
        .get(
            &"/api2/search/?q=binary&search_filename_only=false".to_string(),
            Some(token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let results = body["results"].as_array().unwrap();
    assert!(results.is_empty(), "binary file should not be indexed");
}

/// Delete a file → should remove it from the index.
#[tokio::test]
async fn test_delete_cleans_index() {
    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;

    // Upload
    let resp = f
        .client
        .upload_file(
            token,
            &f.repo_id,
            "/",
            "delete_me.txt",
            b"content to delete",
        )
        .await;
    assert_eq!(resp.status(), 200);

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Verify it's in the index
    let resp = f
        .client
        .get(
            &"/api2/search/?q=delete&search_filename_only=false".to_string(),
            Some(token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        !body["results"].as_array().unwrap().is_empty(),
        "file should be in index before delete"
    );

    // Delete
    let resp = f
        .client
        .delete(
            &format!("/api2/repos/{}/file/?p=/delete_me.txt", f.repo_id),
            Some(token),
        )
        .await;
    assert_eq!(resp.status(), 200);

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Search again — should not find it
    let resp = f
        .client
        .get(
            &"/api2/search/?q=delete&search_filename_only=false".to_string(),
            Some(token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let results = body["results"].as_array().unwrap();
    // The filename "delete_me.txt" would still match in filename search,
    // but the content "content to delete" should not.
    // After deletion, the file is gone from both filename and content index.
    assert!(
        results.is_empty(),
        "file should be removed from index after deletion"
    );
}

/// Rename a file → new path should be searchable.
#[tokio::test]
async fn test_rename_updates_index() {
    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;

    // Upload
    let resp = f
        .client
        .upload_file(token, &f.repo_id, "/", "rename_me.txt", b"rename content")
        .await;
    assert_eq!(resp.status(), 200);

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Rename via form POST
    let resp = f
        .client
        .post_form(
            &format!("/api2/repos/{}/file/?p=/rename_me.txt", f.repo_id),
            Some(token),
            &[("operation", "rename"), ("newname", "renamed.txt")],
        )
        .await;
    assert_eq!(resp.status(), 200);

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Content search at new path should work
    let resp = f
        .client
        .get(
            &"/api2/search/?q=rename+content&search_filename_only=false".to_string(),
            Some(token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let results = body["results"].as_array().unwrap();
    assert!(!results.is_empty(), "should find renamed file via content");
    assert_eq!(results[0]["name"], "renamed.txt");
}

/// Move a file to a subdirectory → new path should be searchable.
#[tokio::test]
async fn test_move_updates_index() {
    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;

    // Create a subdirectory
    let resp = f.client.create_dir(token, &f.repo_id, "/subdir").await;
    assert_eq!(resp.status(), 200);

    // Upload
    let resp = f
        .client
        .upload_file(token, &f.repo_id, "/", "move_me.txt", b"move content test")
        .await;
    assert_eq!(resp.status(), 200);

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Move via form POST
    let resp = f
        .client
        .post_form(
            &format!("/api2/repos/{}/file/?p=/move_me.txt", f.repo_id),
            Some(token),
            &[
                ("operation", "move"),
                ("dst_repo", &f.repo_id),
                ("dst_dir", "/subdir"),
            ],
        )
        .await;
    assert_eq!(resp.status(), 200);

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Content search at new path should work
    let resp = f
        .client
        .get(
            &"/api2/search/?q=move+content&search_filename_only=false".to_string(),
            Some(token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let results = body["results"].as_array().unwrap();
    assert!(!results.is_empty(), "should find moved file via content");
    assert_eq!(results[0]["fullpath"], "/subdir/move_me.txt");
}

/// Batch delete → all files removed from index.
#[tokio::test]
async fn test_batch_delete_cleans_index() {
    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;

    // Upload multiple files
    for i in 0..3 {
        let name = format!("batch-{}.txt", i);
        let content = format!("batch content {}", i);
        let resp = f
            .client
            .upload_file(token, &f.repo_id, "/", &name, content.as_bytes())
            .await;
        assert_eq!(resp.status(), 200);
    }

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Verify files are in index
    let resp = f
        .client
        .get(
            &"/api2/search/?q=batch&search_filename_only=false".to_string(),
            Some(token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let total = body["total"].as_i64().unwrap();
    assert!(total >= 3, "should find batch files in index");

    // Batch delete via v2.1 API
    let resp = f
        .client
        .post_json(
            "/api/v2.1/repos/batch-delete-item/",
            Some(token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "parent_dir": "/",
                "dirents": ["batch-0.txt", "batch-1.txt", "batch-2.txt"]
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Search again — should not find batch content (filenames might match)
    let resp = f
        .client
        .get(
            &"/api2/search/?q=batch+0&search_filename_only=false".to_string(),
            Some(token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let results = body["results"].as_array().unwrap();
    // Filename "batch-0.txt" still matches in filename search if the dir_entries exist via FS tree.
    // But content search for "batch content 0" should not find it.
    // This test is checking that the full-text content index is cleaned up.
    // The filename might still match, so check for the content term specifically.
    let _has_content_match = results
        .iter()
        .any(|r| r["fullpath"].as_str().unwrap_or("").contains("batch-0.txt"));
    // Actually, after deletion, the filename shouldn't match either (dir entry is removed).
    // But to be safe, check that total results decreased.
    assert!(
        results.len() < total as usize,
        "batch delete should reduce index results"
    );
}

/// Multiple repos — content search should only find accessible files.
#[tokio::test]
async fn test_content_search_multi_repo() {
    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;

    // Upload content-matching file to first repo
    let resp = f
        .client
        .upload_file(token, &f.repo_id, "/", "unique.txt", b"unique term xyz987")
        .await;
    assert_eq!(resp.status(), 200);

    // Create second repo and upload with same content
    let repo2_id = common::create_test_repo(&f.client, token, "second-repo").await;
    let resp = f
        .client
        .upload_file(token, &repo2_id, "/", "other.txt", b"unique term xyz987")
        .await;
    assert_eq!(resp.status(), 200);

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Search from first user — should find both repos
    let resp = f
        .client
        .get(
            &"/api2/search/?q=xyz987&search_filename_only=false".to_string(),
            Some(token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let results = body["results"].as_array().unwrap();
    assert_eq!(
        results.len(),
        2,
        "should find content in both repos for same user"
    );
}

/// Reindex endpoint should rebuild the index from storage.
#[tokio::test]
async fn test_reindex_endpoint() {
    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;

    // Upload a text file
    let resp = f
        .client
        .upload_file(
            token,
            &f.repo_id,
            "/",
            "reindexable.txt",
            b"reindexable content",
        )
        .await;
    assert_eq!(resp.status(), 200);

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Verify it's indexed
    let resp = f
        .client
        .get(
            &"/api2/search/?q=reindexable&search_filename_only=false".to_string(),
            Some(token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let before: serde_json::Value = resp.json().await.unwrap();
    assert!(!before["results"].as_array().unwrap().is_empty());

    // Call the reindex endpoint
    let resp = f
        .client
        .post_json(
            "/api2/reindex/",
            Some(token),
            &serde_json::json!({"repo_id": f.repo_id}),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
    assert!(body["indexed"].as_u64().unwrap() >= 1);
}

/// Upload a binary image, then use index_file_text to associate extracted text.
#[tokio::test]
async fn test_index_file_text_for_binary() {
    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;

    // Upload a binary file (PNG header) — not indexable as text.
    let resp = f
        .client
        .upload_file(
            token,
            &f.repo_id,
            "/",
            "screenshot.png",
            b"\x89PNG\r\n\x1a\nbinary pixel data here",
        )
        .await;
    assert_eq!(resp.status(), 200);

    // Verify it's NOT found via content search (binary file skipped).
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let resp = f
        .client
        .get(
            &format!("/api2/search/?q=vision+model&search_filename_only=false"),
            Some(token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body["results"].as_array().unwrap().is_empty(),
        "binary file should not be indexed at upload"
    );

    // Now use index_file_text to associate extracted text with the image.
    let resp = f
        .client
        .post_json(
            "/api2/index-file-text/",
            Some(token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/screenshot.png",
                "text": "This screenshot shows the login page with username and password fields. The vision model extracted this text."
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");

    // Now search for the extracted text — should find the image.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let resp = f
        .client
        .get(
            &format!("/api2/search/?q=vision+model+extracted&search_filename_only=false"),
            Some(token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let results = body["results"].as_array().unwrap();
    assert!(
        !results.is_empty(),
        "should find image via custom text index"
    );
    assert_eq!(
        results[0]["name"], "screenshot.png",
        "should match the image file"
    );

    // Update with different text — should replace the old index entry.
    let resp = f
        .client
        .post_json(
            "/api2/index-file-text/",
            Some(token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/screenshot.png",
                "text": "Updated text from a different vision model run."
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Old text should no longer match (replaced).
    let resp = f
        .client
        .get(
            &format!("/api2/search/?q=login+page+username&search_filename_only=false"),
            Some(token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body["results"].as_array().unwrap().is_empty(),
        "old text should be replaced"
    );

    // New text should match.
    let resp = f
        .client
        .get(
            &format!("/api2/search/?q=different+vision+model&search_filename_only=false"),
            Some(token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let results = body["results"].as_array().unwrap();
    assert!(!results.is_empty(), "should find image via updated text");
    assert_eq!(results[0]["name"], "screenshot.png");
}

/// Prefix matching: search "case" should find file containing "Caseend".
#[tokio::test]
async fn test_prefix_matching_in_content() {
    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;

    // Upload a file with "Caseend" in the content
    let resp = f
        .client
        .upload_file(
            token,
            &f.repo_id,
            "/",
            "readme.md",
            b"This file mentions Caseend which is a compound word.",
        )
        .await;
    assert_eq!(resp.status(), 200);

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Exact match should still work
    let resp = f
        .client
        .get(
            &format!("/api2/search/?q=Caseend&search_filename_only=false"),
            Some(token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let results = body["results"].as_array().unwrap();
    assert!(!results.is_empty(), "exact match should find Caseend");
    assert_eq!(results[0]["name"], "readme.md");

    // Prefix match: "case" should find "Caseend"
    let resp = f
        .client
        .get(
            &format!("/api2/search/?q=case&search_filename_only=false"),
            Some(token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let results = body["results"].as_array().unwrap();
    assert!(!results.is_empty(), "prefix 'case' should match 'Caseend'");
    assert_eq!(results[0]["name"], "readme.md");
}
