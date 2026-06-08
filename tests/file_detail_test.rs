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
    assert!(
        body["last_modified"].as_i64().unwrap() > 0,
        "last_modified should be positive"
    );
    assert!(
        body["last_modifier_name"].as_str().is_some(),
        "last_modifier_name should exist"
    );
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
