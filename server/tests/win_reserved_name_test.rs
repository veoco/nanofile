//! Integration tests for Windows reserved device names.
//!
//! Windows clients (NTFS) cannot materialize names like `CON`, `NUL`, `COM1`
//! or any of their extension variants (`CON.txt`), so the server rejects
//! creating such entries at the upload / mkdir / rename boundaries with 400.

use crate::common;

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

/// Win32 forbids `: * ? |` in file names, so a synced entry containing one can
/// never be materialized by a Windows client: reject it at every creation
/// boundary (upload, mkdir, rename).
#[tokio::test]
async fn test_windows_forbidden_characters_rejected() {
    let f = TestFixture::new().await;

    for name in ["report:2024.txt", "what?.txt", "star*.txt", "pipe|name.txt"] {
        let resp = f
            .client
            .upload_file(&f.api_token, &f.repo_id, "/", name, b"x")
            .await;
        assert_eq!(resp.status(), 400, "upload of {name:?} must be rejected");
    }

    let resp = f
        .client
        .create_dir(&f.api_token, &f.repo_id, "/bad:dir")
        .await;
    assert_eq!(resp.status(), 400, "mkdir of ':' name must be rejected");

    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "original.txt", b"x")
        .await;
    assert!(resp.status().is_success());
    let resp = f
        .client
        .rename_file(&f.api_token, &f.repo_id, "/original.txt", "renamed?.txt")
        .await;
    assert_eq!(resp.status(), 400, "rename to an invalid name is rejected");
}

/// Win32 silently strips trailing dots and spaces, so an entry named `a.` or
/// `a ` cannot round-trip: the server refuses to create one.
#[tokio::test]
async fn test_trailing_dot_or_space_rejected() {
    let f = TestFixture::new().await;

    for name in ["trailing.", "trailing "] {
        let resp = f
            .client
            .upload_file(&f.api_token, &f.repo_id, "/", name, b"x")
            .await;
        assert_eq!(
            resp.status(),
            400,
            "upload of trailing dot/space {name:?} must be rejected"
        );
    }

    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "original.txt", b"x")
        .await;
    assert!(resp.status().is_success());
    let resp = f
        .client
        .rename_file(&f.api_token, &f.repo_id, "/original.txt", "renamed.")
        .await;
    assert_eq!(resp.status(), 400, "rename to a trailing dot is rejected");

    // Parentheses and embedded dots/spaces are part of ordinary conflict and
    // document names, so they must keep working.
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "report (1).final.txt", b"x")
        .await;
    assert!(
        resp.status().is_success(),
        "ordinary punctuation must stay accepted: {}",
        resp.status()
    );
}
