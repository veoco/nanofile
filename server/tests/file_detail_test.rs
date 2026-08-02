mod common;

use common::TestFixture;

#[tokio::test]
async fn test_file_detail_success() {
    let f = TestFixture::new().await;

    // Upload a file first
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "test.txt", b"hello world")
        .await;
    assert!(
        resp.status().is_success(),
        "upload failed: {}",
        resp.status()
    );

    // Get file detail
    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/file/detail/?p=/test.txt", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "test.txt");
    assert_eq!(body["type"], "file");
    assert_eq!(body["size"], 11);
    assert!(
        body["id"].as_str().unwrap().len() == 40,
        "expected 40-char fs_id, got: {}",
        body["id"]
    );
    // last_modified must be an ISO 8601 offset datetime string (seahub /
    // Android FileDetailModel contract), not a raw Unix timestamp.
    let last_modified = body["last_modified"]
        .as_str()
        .expect("last_modified should be a string");
    let parsed = chrono::DateTime::parse_from_rfc3339(last_modified)
        .expect("last_modified should parse as RFC3339/ISO8601");
    assert!(parsed.timestamp() > 0, "last_modified should be positive");
    assert!(
        body["last_modifier_name"].as_str().is_some(),
        "last_modifier_name should exist"
    );
    assert_eq!(body["permission"], "rw");
}

#[tokio::test]
async fn test_file_detail_not_found() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/file/detail/?p=/nonexistent.txt", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn test_file_detail_directory_returns_400() {
    let f = TestFixture::new().await;

    // Create a dir
    let resp = f
        .client
        .create_dir(&f.api_token, &f.repo_id, "/subdir")
        .await;
    assert!(resp.status().is_success());

    // Requesting detail on a directory should fail
    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/file/detail/?p=/subdir", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn test_file_detail_unauthorized() {
    let server = common::TestServer::start().await;
    let client = server.client();

    let resp = client
        .get("/api2/repos/some-repo/file/detail/?p=/test.txt", None)
        .await;
    assert_eq!(resp.status(), 401);
}
