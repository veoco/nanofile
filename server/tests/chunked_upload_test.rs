mod common;

use common::TestFixture;

// ======================================================================
// Upload / Update Blocks Link Tests
// ======================================================================

#[tokio::test]
async fn test_upload_blks_link_returns_url() {
    let f = TestFixture::new().await;

    let resp = f.client.upload_blks_link(&f.api_token, &f.repo_id).await;
    assert_eq!(
        resp.status(),
        200,
        "upload-blks-link failed: {:?}",
        resp.text().await
    );

    let body: String = resp.json().await.unwrap();
    assert!(
        body.contains("upload-blks-api/"),
        "response should contain upload-blks-api/ URL, got: {body}"
    );
    assert!(
        body.starts_with("http://"),
        "response should start with http://, got: {body}"
    );
}

#[tokio::test]
async fn test_update_blks_link_returns_url() {
    let f = TestFixture::new().await;

    let resp = f.client.update_blks_link(&f.api_token, &f.repo_id).await;
    assert_eq!(
        resp.status(),
        200,
        "update-blks-link failed: {:?}",
        resp.text().await
    );

    let body: String = resp.json().await.unwrap();
    assert!(
        body.contains("update-blks-api/"),
        "response should contain update-blks-api/ URL, got: {body}"
    );
    assert!(
        body.starts_with("http://"),
        "response should start with http://, got: {body}"
    );
}

#[tokio::test]
async fn test_upload_blks_link_no_auth() {
    let f = TestFixture::new().await;

    let resp = f.client.upload_blks_link("invalid-token", &f.repo_id).await;
    assert!(
        resp.status() == 401 || resp.status() == 403,
        "expected 401 or 403, got {}",
        resp.status()
    );
}

#[tokio::test]
async fn test_upload_blks_link_nonexistent_repo() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .upload_blks_link(&f.api_token, "nonexistent-repo-id")
        .await;
    assert_eq!(resp.status(), 404);
}

// ======================================================================
// File Uploaded Bytes Tests
// ======================================================================

#[tokio::test]
async fn test_file_uploaded_bytes_stub() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .file_uploaded_bytes(&f.api_token, &f.repo_id, "test.txt", "/")
        .await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["uploadedBytes"].as_i64(), Some(0));
}

#[tokio::test]
async fn test_file_uploaded_bytes_missing_params() {
    let f = TestFixture::new().await;

    // Missing file_name should fail
    let resp = f
        .client
        .file_uploaded_bytes(&f.api_token, &f.repo_id, "", "")
        .await;
    assert_eq!(resp.status(), 400);
}

/// Security: upload-blks commit mode must reject non-hex / path-traversal
/// block ids with a 400 instead of storing them in the fs object.
#[tokio::test]
async fn test_upload_blks_commit_rejects_malicious_blockids() {
    let f = TestFixture::new().await;

    let resp = f.client.upload_blks_link(&f.api_token, &f.repo_id).await;
    assert_eq!(resp.status(), 200);
    let url: String = resp.json().await.unwrap();
    assert!(url.contains("upload-blks-api/"), "bad url: {url}");

    let form = reqwest::multipart::Form::new()
        .text("commitonly", "1")
        .text("parent_dir", "/")
        .text("file_name", "pwned.txt")
        .text("file_size", "6")
        .text("blockids", r#"["../../../../etc/passwd"]"#);
    let resp = f.client.post_multipart_url(&url, form).await;
    assert_eq!(resp.status(), 400);
}
