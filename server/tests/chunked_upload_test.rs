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

/// Build a multipart body for the no-token `/upload-aj/` chunk endpoint.
fn chunked_upload_form(repo_id: &str, bytes: Vec<u8>) -> reqwest::multipart::Form {
    let part = reqwest::multipart::Part::bytes(bytes).file_name("big.txt");
    reqwest::multipart::Form::new()
        .part("file", part)
        .text("repo_id", repo_id.to_string())
        .text("parent_dir", "/")
        .text("relative_path", "")
}

/// Mint a short-lived upload-aj token for the fixture's repo via the Seahub
/// upload-link endpoint (`GET /api2/repos/{repo}/upload-link/?from=web`).
async fn get_upload_token(f: &TestFixture) -> String {
    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/upload-link/?from=web", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200, "upload-link request failed");
    let url: String = resp.json().await.unwrap();
    url.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap()
        .to_string()
}

/// End-to-end Content-Range chunked upload: an intermediate chunk returns
/// `{"success": true}` without committing, and the final chunk assembles the
/// temp file and commits a complete file whose bytes round-trip exactly.
#[tokio::test]
async fn test_chunked_upload_assembles_and_commits() {
    let f = TestFixture::new().await;
    let base = f.server.base_url.clone();
    let repo_id = f.repo_id.clone();

    // Token-authenticated chunk upload via /upload-aj/{token}/.
    let token = get_upload_token(&f).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();

    let content = b"hello world!";
    let split = 6;
    let (c1, c2) = content.split_at(split);

    // Chunk 1 (intermediate) — written to the temp file, not committed.
    let resp = client
        .post(format!("{}/upload-aj/{}", base, token))
        .header(
            "content-range",
            format!("bytes 0-{}/{}", split - 1, content.len()),
        )
        .multipart(chunked_upload_form(&repo_id, c1.to_vec()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["success"], true,
        "intermediate chunk should not commit"
    );

    // Chunk 2 (final) — assembles and commits.
    let resp = client
        .post(format!("{}/upload-aj/{}", base, token))
        .header(
            "content-range",
            format!("bytes {}-{}/{}", split, content.len() - 1, content.len()),
        )
        .multipart(chunked_upload_form(&repo_id, c2.to_vec()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.is_array(), "final chunk should return the file list");
    assert_eq!(body[0]["name"], "big.txt");
    assert_eq!(body[0]["size"], content.len() as i64);

    // The assembled file must round-trip the exact bytes.
    let resp = f
        .client
        .download_file(&f.api_token, &repo_id, "/big.txt")
        .await;
    assert_eq!(resp.status(), 200);
    let downloaded = resp.bytes().await.unwrap();
    assert_eq!(downloaded.as_ref(), content);
}

/// Resume: after sending chunk 1, the client can re-send chunk 1 (e.g. after
/// a dropped connection) and the temp file is overwritten at the same offset
/// before the final chunk commits.
#[tokio::test]
async fn test_chunked_upload_resume_after_interrupt() {
    let f = TestFixture::new().await;
    let base = f.server.base_url.clone();
    let repo_id = f.repo_id.clone();

    let token = get_upload_token(&f).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();

    let content = b"resumed-content";
    let split = 8;

    // First attempt: chunk 1 with wrong data, then re-send the correct chunk 1.
    for data in [b"WRONG!!!", &content[..split]] {
        let resp = client
            .post(format!("{}/upload-aj/{}", base, token))
            .header(
                "content-range",
                format!("bytes 0-{}/{}", split - 1, content.len()),
            )
            .multipart(chunked_upload_form(&repo_id, data.to_vec()))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
    }

    // Final chunk commits the resumed file.
    let resp = client
        .post(format!("{}/upload-aj/{}", base, token))
        .header(
            "content-range",
            format!("bytes {}-{}/{}", split, content.len() - 1, content.len()),
        )
        .multipart(chunked_upload_form(&repo_id, content[split..].to_vec()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body[0]["size"], content.len() as i64);

    let resp = f
        .client
        .download_file(&f.api_token, &repo_id, "/big.txt")
        .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.bytes().await.unwrap().as_ref(), content);
}

/// Security: a chunk whose byte length doesn't match its declared
/// Content-Range span must be rejected with a 400 (not silently truncated).
#[tokio::test]
async fn test_chunked_upload_rejects_size_mismatch() {
    let f = TestFixture::new().await;
    let base = f.server.base_url.clone();
    let repo_id = f.repo_id.clone();

    let token = get_upload_token(&f).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();

    // Header says 4 bytes (0-3/8) but the body carries 2 bytes.
    let resp = client
        .post(format!("{}/upload-aj/{}", base, token))
        .header("content-range", "bytes 0-3/8")
        .multipart(chunked_upload_form(&repo_id, b"ab".to_vec()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "chunk size mismatch must be rejected");
}

/// Security: upload-blks commit mode must reject path-traversal / invalid
/// file names (aligned with seafile's should_ignore_file semantics).
#[tokio::test]
async fn test_upload_blks_commit_rejects_invalid_filename() {
    let f = TestFixture::new().await;

    let resp = f.client.upload_blks_link(&f.api_token, &f.repo_id).await;
    assert_eq!(resp.status(), 200);
    let url: String = resp.json().await.unwrap();

    for bad_name in ["../evil.txt", "a/b.txt", "..", "not\x00null"] {
        let form = reqwest::multipart::Form::new()
            .text("commitonly", "1")
            .text("parent_dir", "/")
            .text("file_name", bad_name)
            .text("file_size", "6")
            .text(
                "blockids",
                r#"["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]"#,
            );
        let resp = f.client.post_multipart_url(&url, form).await;
        assert_eq!(
            resp.status(),
            400,
            "file_name {bad_name:?} should be rejected"
        );
    }
}
