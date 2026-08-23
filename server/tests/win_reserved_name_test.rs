//! Integration tests for Windows reserved device names.
//!
//! Windows clients (NTFS) cannot materialize names like `CON`, `NUL`, `COM1`
//! or any of their extension variants (`CON.txt`), so the server rejects
//! creating such entries at the upload / mkdir / rename boundaries with 400.

mod common;

use common::TestFixture;

#[tokio::test]
async fn test_upload_reserved_name_rejected() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "CON.txt", b"x")
        .await;
    assert_eq!(
        resp.status(),
        400,
        "upload of reserved name must be rejected"
    );
}

#[tokio::test]
async fn test_mkdir_reserved_name_rejected() {
    let f = TestFixture::new().await;

    let resp = f.client.create_dir(&f.api_token, &f.repo_id, "/NUL").await;
    assert_eq!(
        resp.status(),
        400,
        "mkdir of reserved name must be rejected"
    );
}

#[tokio::test]
async fn test_normal_names_still_work() {
    let f = TestFixture::new().await;

    // Files that merely *contain* reserved substrings must not be affected.
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "report.txt", b"hello")
        .await;
    assert!(
        resp.status().is_success(),
        "normal upload should succeed: {}",
        resp.status()
    );

    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "console.log", b"log")
        .await;
    assert!(
        resp.status().is_success(),
        "upload containing 'con' substring should succeed: {}",
        resp.status()
    );

    let resp = f
        .client
        .create_dir(&f.api_token, &f.repo_id, "/archive")
        .await;
    assert!(
        resp.status().is_success(),
        "normal mkdir should succeed: {}",
        resp.status()
    );
}
