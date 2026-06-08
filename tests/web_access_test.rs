mod common;

use common::TestFixture;

/// E.1.4 — GET /f/{token} — shared file link download
#[tokio::test]
async fn test_shared_file_link_not_found() {
    let server = common::TestServer::start().await;
    let client = server.client();
    let resp = client.get("/f/invalid-token/", None).await;
    assert_eq!(resp.status(), 404);
}

/// E.1.5 — GET /repos/{repo_id}/files/{path}
#[tokio::test]
async fn test_repo_files_download() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "test.txt", b"hello web")
        .await;
    assert!(resp.status().is_success());

    let resp = f
        .client
        .get(
            &format!("/repos/{}/files/test.txt", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body = resp.bytes().await.unwrap();
    assert_eq!(&body[..], b"hello web");
}

#[tokio::test]
async fn test_repo_files_unauthorized() {
    let server = common::TestServer::start().await;
    let client = server.client();
    let resp = client.get("/repos/some-repo/files/test.txt", None).await;
    assert_eq!(resp.status(), 401);
}

/// E.2.1 — POST /upload-aj/ — AJAX file upload (multipart)
#[tokio::test]
async fn test_upload_aj() {
    let f = TestFixture::new().await;

    let file_part = reqwest::multipart::Part::bytes(b"test content".to_vec())
        .file_name("web-test.txt".to_string())
        .mime_str("text/plain")
        .unwrap();
    let form = reqwest::multipart::Form::new()
        .part("file", file_part)
        .text("repo_id", f.repo_id.clone())
        .text("parent_dir", "/");

    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let resp = client
        .post(format!("{}/upload-aj/", f.server.base_url))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

/// E.3.1 — GET /upload_progress
#[tokio::test]
async fn test_upload_progress() {
    let f = TestFixture::new().await;
    let resp = f.client.get("/upload_progress", Some(&f.api_token)).await;
    assert_eq!(resp.status(), 200);
}

/// E.3.2 — GET /idx_progress
#[tokio::test]
async fn test_idx_progress() {
    let f = TestFixture::new().await;
    let resp = f.client.get("/idx_progress", Some(&f.api_token)).await;
    assert_eq!(resp.status(), 200);
}

/// E.2.6 — POST /update-aj/ — AJAX file update
#[tokio::test]
async fn test_update_aj() {
    let f = TestFixture::new().await;

    let file_part = reqwest::multipart::Part::bytes(b"updated".to_vec())
        .file_name("update.txt".to_string())
        .mime_str("text/plain")
        .unwrap();
    let form = reqwest::multipart::Form::new()
        .part("file", file_part)
        .text("repo_id", f.repo_id.clone())
        .text("target_file", "/update.txt");

    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let resp = client
        .post(format!("{}/update-aj/", f.server.base_url))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

// ─── upload-api / update-api token-based upload tests ──────────────────

/// F.1 — GET /api2/repos/{id}/upload-link/ returns a JSON string URL.
#[tokio::test]
async fn test_get_upload_link_returns_url() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/upload-link/?p=/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: String = resp.json().await.unwrap();
    assert!(
        body.starts_with("http://127.0.0.1:"),
        "expected full URL, got: {body}"
    );
    assert!(
        body.contains("/upload-api/"),
        "expected upload-api URL, got: {body}"
    );
}

/// F.2 — GET /api2/repos/{id}/update-link/ returns a JSON string URL.
#[tokio::test]
async fn test_get_update_link_returns_url() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/update-link/?p=/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: String = resp.json().await.unwrap();
    assert!(
        body.starts_with("http://127.0.0.1:"),
        "expected full URL, got: {body}"
    );
    assert!(
        body.contains("/update-api/"),
        "expected update-api URL, got: {body}"
    );
}

/// F.3 — POST /upload-api/{token} uploads a file successfully.
#[tokio::test]
async fn test_upload_api_uploads_file() {
    let f = TestFixture::new().await;

    // Get upload link
    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/upload-link/?p=/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    let upload_url: String = resp.json().await.unwrap();

    // Upload file via the token URL
    let file_part = reqwest::multipart::Part::bytes(b"upload api test".to_vec())
        .file_name("api-test.txt".to_string());
    let form = reqwest::multipart::Form::new()
        .part("file", file_part)
        .text("parent_dir", "/");
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let resp = client
        .post(&upload_url)
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Verify via file detail endpoint
    let detail = f
        .client
        .get(
            &format!("/api2/repos/{}/file/detail/?p=/api-test.txt", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(detail.status(), 200);
}

/// F.4 — POST /upload-api/{token} with invalid token returns 400.
#[tokio::test]
async fn test_upload_api_invalid_token() {
    let f = TestFixture::new().await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let resp = client
        .post(format!(
            "{}/upload-api/invalid-token-xxx",
            f.server.base_url
        ))
        .multipart(reqwest::multipart::Form::new())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

/// F.5 — POST /upload-api/{token} to a Chinese-named subdirectory.
#[tokio::test]
async fn test_upload_api_to_chinese_dir() {
    let f = TestFixture::new().await;

    // Create a Chinese-named directory
    let dir_resp = f
        .client
        .post_form(
            &format!("/api2/repos/{}/dir/?p=/未命名", f.repo_id),
            Some(&f.api_token),
            &[("operation", "mkdir")],
        )
        .await;
    assert_eq!(dir_resp.status(), 200, "create chinese dir failed");

    // Get upload link for that directory
    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/upload-link/?p=/未命名", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let upload_url: String = resp.json().await.unwrap();

    // Upload file
    let file_part = reqwest::multipart::Part::bytes(b"chinese dir test".to_vec())
        .file_name("hello.txt".to_string());
    let form = reqwest::multipart::Form::new()
        .part("file", file_part)
        .text("parent_dir", "/未命名");
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let resp = client
        .post(&upload_url)
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "upload to chinese dir failed");

    // Verify file exists via detail endpoint
    let detail = f
        .client
        .get(
            &format!("/api2/repos/{}/file/detail/?p=/未命名/hello.txt", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(
        detail.status(),
        200,
        "file not found after chinese dir upload"
    );
}

/// F.6a — GET /api/v2.1/repos/{repo_id}/file-uploaded-bytes/ returns 0 and Accept-Ranges.
#[tokio::test]
async fn test_file_uploaded_bytes_returns_zero() {
    let f = TestFixture::new().await;

    // Upload a file first so the repo has content
    let up = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "test.txt", b"hello")
        .await;
    assert!(up.status().is_success());

    let resp = f
        .client
        .get(
            &format!(
                "/api/v2.1/repos/{}/file-uploaded-bytes/?file_name=test.txt&parent_dir=/",
                f.repo_id
            ),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let headers = resp.headers().clone();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["uploadedBytes"], 0);
    assert_eq!(
        headers.get("accept-ranges").and_then(|v| v.to_str().ok()),
        Some("bytes")
    );
}

/// F.6b — GET /api/v2.1/repos/{repo_id}/file-uploaded-bytes/ without params returns 400.
#[tokio::test]
async fn test_file_uploaded_bytes_missing_params() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .get(
            &format!("/api/v2.1/repos/{}/file-uploaded-bytes/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 400);
}

/// F.7 — POST /update-api/{token} updates an existing file.
#[tokio::test]
async fn test_update_api_updates_file() {
    let f = TestFixture::new().await;

    // Upload initial file via regular API
    let up = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "update.txt", b"original")
        .await;
    assert!(up.status().is_success(), "initial upload failed");

    // Get update link
    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/update-link/?p=/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    let update_url: String = resp.json().await.unwrap();

    // Update via update-api
    let file_part =
        reqwest::multipart::Part::bytes(b"updated".to_vec()).file_name("update.txt".to_string());
    let form = reqwest::multipart::Form::new()
        .part("file", file_part)
        .text("target_file", "/update.txt");
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let resp = client
        .post(&update_url)
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Download and verify new content
    let dl = f
        .client
        .get(
            &format!("/api2/repos/{}/file/?p=/update.txt", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(dl.status(), 200);
    let body = dl.bytes().await.unwrap();
    assert_eq!(
        &body[..],
        b"updated",
        "file content not updated via update-api"
    );
}

/// F.8 — POST /upload-api/{token} with a quoted boundary (Qt-style Content-Type).
/// Qt's QHttpMultiPart sends `boundary="<value>"` (quoted) in Content-Type but
/// uses unquoted `--<value>` in the body. Without quote stripping, the server
/// would parse the wrong boundary and silently accept the upload without
/// saving the file.
#[tokio::test]
async fn test_upload_api_quoted_boundary() {
    let f = TestFixture::new().await;

    // Get upload link
    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/upload-link/?p=/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    let upload_url: String = resp.json().await.unwrap();

    // Build a minimal multipart body with unquoted boundary markers.
    // The Content-Type will have a QUOTED boundary to simulate Qt's behavior.
    let boundary = "test.seafile.12345";
    let body = format!(
        "--{boundary}\r\n\
         Content-Disposition: form-data; name=\"parent_dir\"\r\n\r\n\
         /\r\n\
         --{boundary}\r\n\
         Content-Disposition: form-data; name=\"file\"; filename=\"qt-test.txt\"\r\n\
         Content-Type: application/octet-stream\r\n\r\n\
         hello from qt client\r\n\
         --{boundary}--\r\n"
    );

    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let resp = client
        .post(&upload_url)
        .header(
            "content-type",
            "multipart/form-data; boundary=\"test.seafile.12345\"",
        )
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "upload with quoted boundary should succeed"
    );

    // Verify file exists
    let detail = f
        .client
        .get(
            &format!("/api2/repos/{}/file/detail/?p=/qt-test.txt", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(
        detail.status(),
        200,
        "file should exist after upload with quoted boundary"
    );
}

/// G.1 — POST /api/v2.1/repos/sync-batch-copy-item/ — same-repo file copy
#[tokio::test]
async fn test_sync_batch_copy_item_same_repo() {
    let f = TestFixture::new().await;

    // Upload a file to root
    let up = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "source.txt", b"hello copy")
        .await;
    assert!(up.status().is_success(), "initial upload failed");

    // Create a subdirectory
    let mkdir = f
        .client
        .create_dir(&f.api_token, &f.repo_id, "/subdir")
        .await;
    assert_eq!(mkdir.status(), 200, "create dir failed");

    // Copy source.txt to /subdir/
    let body = serde_json::json!({
        "src_repo_id": f.repo_id,
        "src_parent_dir": "/",
        "src_dirents": ["source.txt"],
        "dst_repo_id": f.repo_id,
        "dst_parent_dir": "/subdir",
    });
    let resp = f
        .client
        .post_json(
            "/api/v2.1/repos/sync-batch-copy-item/",
            Some(&f.api_token),
            &body,
        )
        .await;
    assert_eq!(resp.status(), 200, "same-repo copy should succeed");

    // Verify original file still exists
    let orig = f
        .client
        .download_file(&f.api_token, &f.repo_id, "/source.txt")
        .await;
    assert_eq!(orig.status(), 200, "original file should still exist");
    let orig_body = orig.bytes().await.unwrap();
    assert_eq!(&orig_body[..], b"hello copy");

    // Verify copied file exists in destination
    let copied = f
        .client
        .download_file(&f.api_token, &f.repo_id, "/subdir/source.txt")
        .await;
    assert_eq!(
        copied.status(),
        200,
        "copied file should exist in destination"
    );
    let copied_body = copied.bytes().await.unwrap();
    assert_eq!(&copied_body[..], b"hello copy", "content should match");
}

/// G.2 — POST /api/v2.1/repos/sync-batch-copy-item/ — cross-repo returns error
#[tokio::test]
async fn test_sync_batch_copy_item_cross_repo_fails() {
    let f = TestFixture::new().await;

    let up = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "source.txt", b"test")
        .await;
    assert!(up.status().is_success());

    // Create a second repo
    let repo2_id = {
        let resp = f.client.create_repo(&f.api_token, "repo2").await;
        assert_eq!(resp.status(), 201);
        let body: serde_json::Value = resp.json().await.unwrap();
        body["id"].as_str().unwrap().to_string()
    };

    // Try cross-repo copy (src_repo != dst_repo)
    let body = serde_json::json!({
        "src_repo_id": f.repo_id,
        "src_parent_dir": "/",
        "src_dirents": ["source.txt"],
        "dst_repo_id": repo2_id,
        "dst_parent_dir": "/",
    });
    let resp = f
        .client
        .post_json(
            "/api/v2.1/repos/sync-batch-copy-item/",
            Some(&f.api_token),
            &body,
        )
        .await;
    assert_eq!(resp.status(), 400, "cross-repo copy should return 400");
}

/// G.3 — POST /api/v2.1/repos/sync-batch-copy-item/ — nonexistent source
#[tokio::test]
async fn test_sync_batch_copy_item_not_found() {
    let f = TestFixture::new().await;

    let body = serde_json::json!({
        "src_repo_id": f.repo_id,
        "src_parent_dir": "/",
        "src_dirents": ["nonexistent.txt"],
        "dst_repo_id": f.repo_id,
        "dst_parent_dir": "/",
    });
    let resp = f
        .client
        .post_json(
            "/api/v2.1/repos/sync-batch-copy-item/",
            Some(&f.api_token),
            &body,
        )
        .await;
    assert_eq!(
        resp.status(),
        404,
        "copying nonexistent file should return 404"
    );
}
