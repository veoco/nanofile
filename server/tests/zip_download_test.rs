//! Integration tests for batch zip download.
//!
//! Covers:
//! - Single directory download
//! - Multi-file batch download
//! - Mixed files + directories
//! - File content integrity verification
//! - Empty directory
//! - Error cases: invalid token, empty dirents, nonexistent entry, unauthorized
//! - Deeply nested directory structure

mod common;

use common::{TestFixture, create_test_user};
use std::io::Read;

/// Helper: upload a file to a repo.
async fn upload(f: &TestFixture, dir: &str, name: &str, data: &[u8]) {
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, dir, name, data)
        .await;
    assert!(
        resp.status().is_success(),
        "upload {dir}/{name} failed: {}",
        resp.status()
    );
}

/// Helper: create a directory.
async fn mkdir(f: &TestFixture, path: &str) {
    let resp = f.client.create_dir(&f.api_token, &f.repo_id, path).await;
    assert!(
        resp.status().is_success(),
        "mkdir {path} failed: {}",
        resp.status()
    );
}

/// Helper: request a zip token and return it.
async fn request_zip(f: &TestFixture, parent_dir: &str, dirents: &[&str]) -> String {
    let resp = f
        .client
        .zip_task(&f.api_token, &f.repo_id, parent_dir, dirents)
        .await;
    assert_eq!(resp.status(), 200, "zip-task request failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["zip_token"].as_str().unwrap().to_string();
    assert!(!token.is_empty(), "zip_token should not be empty");
    token
}

/// Helper: download zip and parse it, returning (names, file_map).
fn parse_zip(data: &[u8]) -> (Vec<String>, std::collections::HashMap<String, Vec<u8>>) {
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(data)).expect("should be a valid zip archive");

    let mut names: Vec<String> = archive.file_names().map(|n| n.to_string()).collect();
    names.sort();

    let mut files = std::collections::HashMap::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).unwrap();
        let name = entry.name().to_string();
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf).unwrap();
        files.insert(name, buf);
    }

    (names, files)
}

// ── Tests ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_single_dir_download() {
    let f = TestFixture::new().await;

    mkdir(&f, "/subdir").await;
    upload(&f, "/subdir", "a.txt", b"hello a").await;
    upload(&f, "/subdir", "b.txt", b"hello b").await;

    let zip_token = request_zip(&f, "/", &["subdir"]).await;

    let zip_resp = f.client.zip_download(&zip_token).await;
    assert_eq!(zip_resp.status(), 200);
    assert_eq!(
        zip_resp.headers().get("content-type").unwrap(),
        "application/zip",
    );
    // Verify Content-Disposition contains directory name
    let cd = zip_resp
        .headers()
        .get("content-disposition")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        cd.contains(r#"filename="subdir.zip""#),
        "bad content-disposition: {cd}"
    );

    let (names, files) = parse_zip(&zip_resp.bytes().await.unwrap());
    // Paths include the directory prefix so extraction creates a folder
    assert_eq!(
        names,
        vec!["subdir/a.txt", "subdir/b.txt"],
        "zip entries should match"
    );
    assert_eq!(files["subdir/a.txt"], b"hello a");
    assert_eq!(files["subdir/b.txt"], b"hello b");
}

#[tokio::test]
async fn test_batch_multi_file_download() {
    let f = TestFixture::new().await;

    upload(&f, "/", "f1.txt", b"file one").await;
    upload(&f, "/", "f2.txt", b"file two").await;
    upload(&f, "/", "f3.txt", b"file three").await;

    let zip_token = request_zip(&f, "/", &["f1.txt", "f3.txt"]).await;

    let zip_resp = f.client.zip_download(&zip_token).await;
    assert_eq!(zip_resp.status(), 200);

    let (names, files) = parse_zip(&zip_resp.bytes().await.unwrap());
    assert_eq!(names, vec!["f1.txt", "f3.txt"], "only selected files");
    assert_eq!(files["f1.txt"], b"file one");
    assert_eq!(files["f3.txt"], b"file three");
}

#[tokio::test]
async fn test_mixed_files_and_dirs_download() {
    let f = TestFixture::new().await;

    // Root file
    upload(&f, "/", "root.txt", b"root").await;

    // Subdir with files
    mkdir(&f, "/subdir").await;
    upload(&f, "/subdir", "sa.txt", b"sub a").await;
    upload(&f, "/subdir", "sb.txt", b"sub b").await;

    // Select both root.txt and subdir/
    let zip_token = request_zip(&f, "/", &["root.txt", "subdir"]).await;
    let zip_resp = f.client.zip_download(&zip_token).await;
    assert_eq!(zip_resp.status(), 200);

    let (names, files) = parse_zip(&zip_resp.bytes().await.unwrap());
    // subdir/ files should be prefixed with subdir/
    assert_eq!(names, vec!["root.txt", "subdir/sa.txt", "subdir/sb.txt"]);
    assert_eq!(files["root.txt"], b"root");
    assert_eq!(files["subdir/sa.txt"], b"sub a");
    assert_eq!(files["subdir/sb.txt"], b"sub b");
}

#[tokio::test]
async fn test_zip_content_integrity() {
    let f = TestFixture::new().await;

    // Upload binary content with known bytes
    let content: Vec<u8> = (0..255).cycle().take(1024).collect();
    upload(&f, "/", "data.bin", &content).await;

    let zip_token = request_zip(&f, "/", &["data.bin"]).await;
    let zip_resp = f.client.zip_download(&zip_token).await;
    assert_eq!(zip_resp.status(), 200);

    let (names, files) = parse_zip(&zip_resp.bytes().await.unwrap());
    assert_eq!(names, vec!["data.bin"]);
    assert_eq!(
        files["data.bin"], content,
        "binary content must match exactly"
    );
}

#[tokio::test]
async fn test_empty_dir_download() {
    let f = TestFixture::new().await;

    mkdir(&f, "/empty").await;

    // Empty directory has no files to zip → handler returns 404
    let resp = f
        .client
        .zip_task(&f.api_token, &f.repo_id, "/", &["empty"])
        .await;
    assert_eq!(resp.status(), 404, "empty dir should return 404");
}

#[tokio::test]
async fn test_nested_dir_structure() {
    let f = TestFixture::new().await;

    // Create deeply nested path: a/b/c/d/e.txt
    mkdir(&f, "/a").await;
    mkdir(&f, "/a/b").await;
    mkdir(&f, "/a/b/c").await;
    mkdir(&f, "/a/b/c/d").await;
    upload(&f, "/a/b/c/d", "e.txt", b"deeply nested").await;

    let zip_token = request_zip(&f, "/", &["a"]).await;
    let zip_resp = f.client.zip_download(&zip_token).await;
    assert_eq!(zip_resp.status(), 200);

    let (names, files) = parse_zip(&zip_resp.bytes().await.unwrap());
    // Path should preserve full nesting
    let expected_path = if cfg!(windows) {
        r"a\b\c\d\e.txt"
    } else {
        "a/b/c/d/e.txt"
    };
    assert_eq!(names, vec![expected_path]);
    assert_eq!(files[expected_path], b"deeply nested");
}

#[tokio::test]
async fn test_invalid_token_returns_404() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .zip_download("00000000-0000-0000-0000-000000000000")
        .await;
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn test_empty_dirents_returns_400() {
    let f = TestFixture::new().await;

    let resp = f.client.zip_task(&f.api_token, &f.repo_id, "/", &[]).await;
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn test_nonexistent_entry_returns_404() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .zip_task(&f.api_token, &f.repo_id, "/", &["does-not-exist"])
        .await;
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn test_unauthorized_access_returns_403() {
    let f = TestFixture::new().await;

    // Create a second user who does NOT own the repo
    let _uid2 = create_test_user(&f.server.db, "intruder@test.com", "password").await;
    let resp = f.client.login("intruder@test.com", "password").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let intruder_token = body["token"].as_str().unwrap();

    let resp = f
        .client
        .zip_task(intruder_token, &f.repo_id, "/", &["some-file"])
        .await;
    assert_eq!(resp.status(), 403);
}
