use crate::common;

use common::TestFixture;

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
        "update-blks-link must keep seahub's /update-blks-api/ URL shape, got: {body}"
    );
    assert!(
        body.starts_with("http://"),
        "response should start with http://, got: {body}"
    );
}

/// The URL `update-blks-link` hands out used to have no route. Upstream's
/// update contract names the target with `target_file`, requires `commitonly`
/// in the query, and always replaces the existing file.
#[tokio::test]
async fn test_update_blks_link_url_is_routable() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "updated.txt", b"original")
        .await;
    assert_eq!(resp.status(), 200, "seeding the file failed");

    let resp = f.client.update_blks_link(&f.api_token, &f.repo_id).await;
    assert_eq!(resp.status(), 200);
    let url: String = resp.json().await.unwrap();
    assert!(url.contains("/update-blks-api/"), "bad url: {url}");

    // `commitonly` in the query and `target_file` in the body: seahub's
    // `UpdateBlksLinkView` contract (the C fileserver reads the query too).
    let form = reqwest::multipart::Form::new()
        .text("target_file", "/updated.txt")
        .text("blockids", "[]")
        .text("file_size", "0");
    let resp = f
        .client
        .post_multipart_url(&format!("{url}?commitonly=true"), form)
        .await;
    assert_eq!(
        resp.status(),
        200,
        "the update-blks URL must reach the block endpoint"
    );
    let body = resp.text().await.unwrap();
    assert_eq!(
        body,
        infra::common::EMPTY_SHA1,
        "a zero-byte block commit answers the empty-file sentinel id"
    );

    let resp = f
        .client
        .download_file(&f.api_token, &f.repo_id, "/updated.txt")
        .await;
    assert_eq!(resp.status(), 200);
    assert!(
        resp.bytes().await.unwrap().is_empty(),
        "update-blks must replace the target in place"
    );
}

/// Upstream answers 441 (`SEAF_HTTP_RES_NOT_EXISTS`) when an update's
/// `target_file` does not exist, rather than creating the file.
#[tokio::test]
async fn test_update_blks_commit_requires_existing_target() {
    let f = TestFixture::new().await;

    let resp = f.client.update_blks_link(&f.api_token, &f.repo_id).await;
    assert_eq!(resp.status(), 200);
    let url: String = resp.json().await.unwrap();

    let form = reqwest::multipart::Form::new()
        .text("target_file", "/missing.txt")
        .text("blockids", "[]")
        .text("file_size", "0");
    let resp = f
        .client
        .post_multipart_url(&format!("{url}?commitonly=true"), form)
        .await;
    assert_eq!(resp.status(), 441);
}

/// `replace=1` (the upstream spelling) is honoured by the upload path, and
/// `commitonly` may sit in the query as seahub mints it.
#[tokio::test]
async fn test_upload_blks_commit_honours_replace() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "replace-me.txt", b"original")
        .await;
    assert_eq!(resp.status(), 200, "seeding the file failed");

    let resp = f.client.upload_blks_link(&f.api_token, &f.repo_id).await;
    assert_eq!(resp.status(), 200);
    let url: String = resp.json().await.unwrap();

    let form = reqwest::multipart::Form::new()
        .text("parent_dir", "/")
        .text("file_name", "replace-me.txt")
        .text("blockids", "[]")
        .text("file_size", "0")
        .text("replace", "1");
    let resp = f
        .client
        .post_multipart_url(&format!("{url}?commitonly=true"), form)
        .await;
    assert_eq!(resp.status(), 200, "replace=1 commit failed");

    let resp = f
        .client
        .download_file(&f.api_token, &f.repo_id, "/replace-me.txt")
        .await;
    assert_eq!(resp.status(), 200);
    let body = resp.bytes().await.unwrap();
    assert!(
        body.is_empty(),
        "replace=1 must overwrite the target in place, got {} bytes",
        body.len()
    );
}

/// The iOS large-file chain: POST the client's block list to
/// `upload-blks-link/`, upload the missing blocks to the returned raw URL, then
/// commit through the returned commit URL (seahub's `UploadBlksLinkView.post`).
#[tokio::test]
async fn test_ios_block_upload_chain() {
    let f = TestFixture::new().await;
    let content = b"iOS block upload payload".to_vec();
    let block_id = infra::crypto::fs_id::sha1_hex(&content);

    let link_path = format!("/api2/repos/{}/upload-blks-link/?p=/", f.repo_id);
    let resp = f
        .client
        .post_form(
            &link_path,
            Some(&f.api_token),
            &[("blklist", block_id.as_str())],
        )
        .await;
    assert_eq!(resp.status(), 200, "upload-blks-link POST failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    let raw_url = body["rawblksurl"].as_str().unwrap().to_string();
    let commit_url = body["commiturl"].as_str().unwrap().to_string();
    assert!(
        raw_url.contains("/upload-raw-blks-api/"),
        "bad raw url: {raw_url}"
    );
    assert!(
        commit_url.contains("/upload-blks-api/"),
        "bad commit url: {commit_url}"
    );
    assert!(commit_url.contains("commitonly=true"), "{commit_url}");
    assert!(commit_url.contains("ret-json=true"), "{commit_url}");
    assert_eq!(
        body["blklist"],
        serde_json::json!([block_id.clone()]),
        "the missing list must name the only block"
    );

    // Raw block upload: one `file` part per block, the part filename is the id.
    let part = reqwest::multipart::Part::bytes(content.clone()).file_name(block_id.clone());
    let form = reqwest::multipart::Form::new().part("file", part);
    let resp = f.client.post_multipart_url(&raw_url, form).await;
    assert_eq!(resp.status(), 200, "raw block upload failed");

    // Commit through the URL seahub hands out (`?commitonly=true&ret-json=true`).
    let form = reqwest::multipart::Form::new()
        .text("parent_dir", "/")
        .text("file_name", "ios-large.bin")
        .text("file_size", content.len().to_string())
        .text(
            "blockids",
            serde_json::json!([block_id.clone()]).to_string(),
        );
    let resp = f.client.post_multipart_url(&commit_url, form).await;
    assert_eq!(resp.status(), 200, "block commit failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body.is_array(),
        "ret-json commit should answer an array: {body}"
    );
    assert_eq!(body[0]["name"], "ios-large.bin");

    let resp = f
        .client
        .download_file(&f.api_token, &f.repo_id, "/ios-large.bin")
        .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.bytes().await.unwrap().as_ref(), content.as_slice());

    // The same call now reports nothing missing.
    let resp = f
        .client
        .post_form(
            &link_path,
            Some(&f.api_token),
            &[("blklist", block_id.as_str())],
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["blklist"], serde_json::json!([]));
}

/// A malformed block id must not reach the store or the committed fs object.
#[tokio::test]
async fn test_ios_block_upload_link_rejects_bad_block_id() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .post_form(
            &format!("/api2/repos/{}/upload-blks-link/?p=/", f.repo_id),
            Some(&f.api_token),
            &[("blklist", "../../etc/passwd")],
        )
        .await;
    assert_eq!(resp.status(), 400);
}

async fn update_link(f: &TestFixture, web: bool) -> String {
    let query = if web { "?p=/&from=web" } else { "?p=/" };
    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/update-link/{}", f.repo_id, query),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    resp.json().await.unwrap()
}

/// An update request without a `file` part must not silently truncate the
/// target (upstream answers `400` from `doUpdate`).
#[tokio::test]
async fn test_update_token_missing_file_does_not_truncate() {
    for web in [false, true] {
        let f = TestFixture::new().await;
        let resp = f
            .client
            .upload_file(&f.api_token, &f.repo_id, "/", "keep.txt", b"original")
            .await;
        assert_eq!(resp.status(), 200);

        let url = update_link(&f, web).await;
        let form = reqwest::multipart::Form::new().text("target_file", "/keep.txt");
        let resp = f.client.post_multipart_url(&url, form).await;
        assert_eq!(resp.status(), 400);

        let resp = f
            .client
            .download_file(&f.api_token, &f.repo_id, "/keep.txt")
            .await;
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.bytes().await.unwrap().as_ref(), b"original");
    }
}

/// The desktop client chunks updates through `/update-api`; that endpoint used
/// to reject every `Content-Range` request.
#[tokio::test]
async fn test_update_token_supports_chunked_update() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "chunked.txt", b"old")
        .await;
    assert_eq!(resp.status(), 200);

    let content = b"chunked update payload";
    for web in [false, true] {
        let url = update_link(&f, web).await;
        let part = reqwest::multipart::Part::bytes(content.to_vec()).file_name("chunked.txt");
        let form = reqwest::multipart::Form::new()
            .text("target_file", "/chunked.txt")
            .part("file", part);
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let resp = client
            .post(&url)
            .header(
                "content-range",
                format!("bytes 0-{}/{}", content.len() - 1, content.len()),
            )
            .multipart(form)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "chunked update failed (web={web})");

        let resp = f
            .client
            .download_file(&f.api_token, &f.repo_id, "/chunked.txt")
            .await;
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.bytes().await.unwrap().as_ref(), content.as_slice());
    }
}

/// Multi-file upload is part of the upstream contract: `-aj` answers a JSON
/// array and `-api` tab-separated ids.
#[tokio::test]
async fn test_multi_file_upload_returns_every_id() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/upload-link/?p=/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    let api_url: String = resp.json().await.unwrap();
    let form = reqwest::multipart::Form::new()
        .text("parent_dir", "/")
        .part(
            "file",
            reqwest::multipart::Part::bytes(b"one".to_vec()).file_name("one.txt"),
        )
        .part(
            "file",
            reqwest::multipart::Part::bytes(b"two".to_vec()).file_name("two.txt"),
        );
    let resp = f.client.post_multipart_url(&api_url, form).await;
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert_eq!(body.split('\t').count(), 2, "got {body:?}");

    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/upload-link/?p=/&from=web", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    let web_url: String = resp.json().await.unwrap();
    let form = reqwest::multipart::Form::new()
        .text("parent_dir", "/")
        .part(
            "file",
            reqwest::multipart::Part::bytes(b"three".to_vec()).file_name("three.txt"),
        )
        .part(
            "file",
            reqwest::multipart::Part::bytes(b"four".to_vec()).file_name("four.txt"),
        );
    let resp = f.client.post_multipart_url(&web_url, form).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body.as_array().unwrap().len(), 2, "got {body}");
}

/// The public upload page posts only the `file` part and relies on the minted
/// token for the directory; a missing `parent_dir` must keep working instead of
/// answering 400 (the `public-upload` e2e spec exercises the same path).
#[tokio::test]
async fn test_upload_token_defaults_to_its_directory() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/upload-link/?p=/subdir&from=web", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let url: String = resp.json().await.unwrap();

    // No parent_dir, no relative_path: exactly what the upload-link page sends.
    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(b"only the file part".to_vec()).file_name("public.txt"),
    );
    let resp = f.client.post_multipart_url(&url, form).await;
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        status, 200,
        "the token's directory must be the default target: {body}"
    );
    assert_eq!(body[0]["name"], "public.txt");

    let resp = f
        .client
        .download_file(&f.api_token, &f.repo_id, "/subdir/public.txt")
        .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.bytes().await.unwrap().as_ref(), b"only the file part");
}

/// The Go fileserver applies `last_modify` to the committed dirent; the
/// upstream desktop/iOS clients send it so the server keeps the local mtime.
#[tokio::test]
async fn test_upload_applies_client_mtime() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/upload-link/?p=/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    let url: String = resp.json().await.unwrap();
    let form = reqwest::multipart::Form::new()
        .text("parent_dir", "/")
        .text("last_modify", "2020-01-02T03:04:05Z")
        .part(
            "file",
            reqwest::multipart::Part::bytes(b"mtime".to_vec()).file_name("mtime.txt"),
        );
    let resp = f.client.post_multipart_url(&url, form).await;
    assert_eq!(resp.status(), 200);

    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/file/detail/?p=/mtime.txt", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["mtime"].as_i64(), Some(1_577_934_245), "got {body}");
}

/// The Go fileserver rejects a leading `/` on `relative_path` and `//` in
/// `parent_dir`; both used to be normalized away.
#[tokio::test]
async fn test_upload_rejects_malformed_paths() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/upload-link/?p=/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    let url: String = resp.json().await.unwrap();

    for (parent, relative) in [("/", "/sub"), ("/a//b", "sub")] {
        let form = reqwest::multipart::Form::new()
            .text("parent_dir", parent.to_string())
            .text("relative_path", relative.to_string())
            .part(
                "file",
                reqwest::multipart::Part::bytes(b"x".to_vec()).file_name("x.txt"),
            );
        let resp = f.client.post_multipart_url(&url, form).await;
        assert_eq!(
            resp.status(),
            400,
            "parent={parent:?} relative={relative:?}"
        );
    }
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

/// Build a multipart body for the `/upload-aj/{token}` chunk endpoint.
fn chunked_upload_form(repo_id: &str, bytes: Vec<u8>) -> reqwest::multipart::Form {
    let part = reqwest::multipart::Part::bytes(bytes).file_name("big.txt");
    reqwest::multipart::Form::new()
        .part("file", part)
        .text("repo_id", repo_id.to_string())
        .text("parent_dir", "/")
        .text("relative_path", "")
}

/// Mint a short-lived upload-aj token for `repo_id` as the user owning
/// `api_token` via the Seahub upload-link endpoint
/// (`GET /api2/repos/{repo}/upload-link/?from=web`).
async fn upload_token_for(
    client: &common::client::TestClient,
    api_token: &str,
    repo_id: &str,
) -> String {
    let resp = client
        .get(
            &format!("/api2/repos/{repo_id}/upload-link/?from=web"),
            Some(api_token),
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

/// Mint a short-lived upload-aj token for the fixture's repo.
async fn get_upload_token(f: &TestFixture) -> String {
    upload_token_for(&f.client, &f.api_token, &f.repo_id).await
}

/// Create an upload link on the fixture repo root and mint one upload token from
/// it — the exact exchange a public upload-link visitor performs.
async fn get_upload_link_visitor_token(f: &TestFixture) -> String {
    let resp = f
        .client
        .post_json(
            "/api/v2.1/upload-links/",
            Some(&f.api_token),
            &serde_json::json!({"repo_id": f.repo_id, "path": "/"}),
        )
        .await;
    assert_eq!(resp.status(), 200, "create upload link failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    let link_token = body["token"].as_str().unwrap().to_string();

    let resp = f
        .client
        .get(
            &format!("/api/v2.1/upload-links/{link_token}/upload/"),
            None,
        )
        .await;
    assert_eq!(resp.status(), 200, "minting the upload token failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    let url = body["upload_link"].as_str().unwrap().to_string();
    url.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap()
        .to_string()
}

/// POST one `Content-Range` chunk to an upload-aj token and return the JSON body.
#[allow(clippy::too_many_arguments)] // a test helper mirroring the chunk header fields
async fn post_chunk(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    repo_id: &str,
    start: usize,
    end: usize,
    total: usize,
    data: &[u8],
) -> serde_json::Value {
    let resp = client
        .post(format!("{base_url}/upload-aj/{token}"))
        .header("content-range", format!("bytes {start}-{end}/{total}"))
        .multipart(chunked_upload_form(repo_id, data.to_vec()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "chunk {start}-{end} failed");
    resp.json().await.unwrap()
}

/// Upload two same-size payloads to the same repo/file path through two
/// credentials, interleaving their chunks, and assert each commits its own bytes.
///
/// Before the temp-upload key was scoped by credential, the second caller was
/// handed the first caller's temp file: the interleaved writes mixed the two
/// payloads and the committed files were corrupted.
async fn assert_interleaved_uploads_are_isolated(f: &TestFixture, token_a: &str, token_b: &str) {
    let base = f.server.base_url.clone();
    let repo_id = f.repo_id.clone();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();

    let a: &[u8] = b"hello world!";
    let b: &[u8] = b"HELLO THERE!";
    assert_eq!(a.len(), b.len());
    let split = 6;

    // Interleave the first (intermediate) chunks for the same target path.
    let reply = post_chunk(
        &client,
        &base,
        token_a,
        &repo_id,
        0,
        split - 1,
        a.len(),
        &a[..split],
    )
    .await;
    assert_eq!(reply["success"], true, "first intermediate chunk: {reply}");
    let reply = post_chunk(
        &client,
        &base,
        token_b,
        &repo_id,
        0,
        split - 1,
        b.len(),
        &b[..split],
    )
    .await;
    assert_eq!(reply["success"], true, "second intermediate chunk: {reply}");

    // Final chunks: each upload must assemble and commit exactly its own bytes.
    let mut committed = Vec::new();
    for (token, data) in [(token_a, a), (token_b, b)] {
        let body = post_chunk(
            &client,
            &base,
            token,
            &repo_id,
            split,
            data.len() - 1,
            data.len(),
            &data[split..],
        )
        .await;
        assert!(
            body.is_array(),
            "final chunk should return the file list: {body}"
        );
        assert_eq!(body[0]["size"], data.len() as i64);
        committed.push((body[0]["name"].as_str().unwrap().to_string(), data.to_vec()));
    }

    // Both uploads created the same target path, so the second keep-both commit
    // is renamed; the point is that each file carries its uploader's bytes.
    assert_eq!(committed[0].0, "big.txt");
    assert_ne!(
        committed[1].0, "big.txt",
        "the second commit of the same path must be renamed"
    );
    for (name, expected) in committed {
        let resp = f
            .client
            .download_file(&f.api_token, &repo_id, &format!("/{name}"))
            .await;
        assert_eq!(resp.status(), 200, "download of /{name} failed");
        assert_eq!(
            resp.bytes().await.unwrap().as_ref(),
            expected.as_slice(),
            "/{name} contains the wrong uploader's bytes"
        );
    }
}

/// Two upload-link visitors minted from two different links are different
/// credentials: their resumable uploads to the same repo/path must not share
/// temp state.
#[tokio::test]
async fn test_chunked_upload_isolated_between_upload_link_visitors() {
    let f = TestFixture::new().await;
    let token_a = get_upload_link_visitor_token(&f).await;
    let token_b = get_upload_link_visitor_token(&f).await;
    assert_interleaved_uploads_are_isolated(&f, &token_a, &token_b).await;
}

/// Two different users writing to the same library/target are different scopes
/// too: interleaved resumable chunks must not overwrite each other's temp file.
#[tokio::test]
async fn test_chunked_upload_isolated_between_users() {
    let f = TestFixture::new().await;

    let other_id =
        common::create_test_user(f.server.db.as_ref(), "other@example.com", "password").await;
    common::add_repo_member(f.server.db.as_ref(), &f.repo_id, other_id, "rw").await;
    let other_client = f.server.client();
    let resp = other_client.login("other@example.com", "password").await;
    assert_eq!(resp.status(), 200, "second user login failed");
    let other_token: String = resp.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    let token_a = get_upload_token(&f).await;
    let token_b = upload_token_for(&other_client, &other_token, &f.repo_id).await;
    assert_interleaved_uploads_are_isolated(&f, &token_a, &token_b).await;
}

/// End-to-end Content-Range chunked upload: an intermediate chunk returns
/// `{"success": true}` without committing, and the final chunk assembles the
/// temp file and commits a complete file whose bytes round-trip exactly.
#[tokio::test]
async fn test_chunked_upload_assembles_and_commits() {
    let f = TestFixture::new().await;
    let base = f.server.base_url.clone();
    let repo_id = f.repo_id.clone();

    let token = get_upload_token(&f).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();

    let content = b"hello world!";
    let split = 6;
    let (c1, c2) = content.split_at(split);

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

/// The temp-upload active cap is enforced end-to-end: a second concurrent
/// resumable upload is rejected with 429 while the first is still in flight
/// (its temp entry persists until the final chunk or the TTL cleanup).
#[tokio::test]
async fn test_chunked_upload_rejects_when_temp_cap_reached() {
    let f = TestFixture::new_with_temp_limits(1).await;
    let base = f.server.base_url.clone();
    let repo_id = f.repo_id.clone();

    let token = get_upload_token(&f).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();

    // Chunk 1 of file A — intermediate (not final), so its temp entry stays
    // active and the second upload must hit the cap.
    let resp = client
        .post(format!("{}/upload-aj/{}", base, token))
        .header("content-range", "bytes 0-5/12")
        .multipart(chunked_upload_form(&repo_id, b"hello ".to_vec()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let form_b = reqwest::multipart::Form::new()
        .part(
            "file",
            reqwest::multipart::Part::bytes(b"hello ".to_vec()).file_name("big2.txt"),
        )
        .text("repo_id", repo_id)
        .text("parent_dir", "/")
        .text("relative_path", "");
    let resp = client
        .post(format!("{}/upload-aj/{}", base, token))
        .header("content-range", "bytes 0-5/12")
        .multipart(form_b)
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        429,
        "second active upload should be rejected"
    );
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

/// Security: commit mode must verify the declared `file_size` against the
/// real block bytes — a low-balled size must not dodge the quota check.
#[tokio::test]
async fn test_upload_blks_commit_rejects_size_lie() {
    let f = TestFixture::new().await;

    let resp = f.client.upload_blks_link(&f.api_token, &f.repo_id).await;
    assert_eq!(resp.status(), 200);
    let url: String = resp.json().await.unwrap();

    let data = b"hello";
    let block_id = {
        use sha1::{Digest, Sha1};
        let mut h = Sha1::new();
        h.update(data);
        hex::encode(h.finalize())
    };
    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(data.to_vec()).file_name(block_id.clone()),
    );
    let resp = f.client.post_multipart_url(&url, form).await;
    assert_eq!(resp.status(), 200, "block upload should succeed");

    let form = reqwest::multipart::Form::new()
        .text("commitonly", "1")
        .text("parent_dir", "/")
        .text("file_name", "real.txt")
        .text("file_size", "3")
        .text("blockids", format!(r#"["{block_id}"]"#));
    let resp = f.client.post_multipart_url(&url, form).await;
    assert_eq!(resp.status(), 400, "size lie must be rejected");
}

/// Security: upload-blks `file` parts must respect the 12 MiB per-block cap so
/// a token holder can't force a multi-GB in-memory buffer per part.
#[tokio::test]
async fn test_upload_blks_block_size_limited() {
    let f = TestFixture::new().await;

    let resp = f.client.upload_blks_link(&f.api_token, &f.repo_id).await;
    assert_eq!(resp.status(), 200);
    let url: String = resp.json().await.unwrap();

    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(vec![0u8; 13 * 1024 * 1024]).file_name("a"),
    );
    // The part is refused as soon as the cap is passed (see
    // `response_or_early_close`): `None` is the connection closing before it was
    // read, which is the same answer as the 413.
    let resp =
        common::client::response_or_early_close(f.client.try_post_multipart_url(&url, form).await);
    if let Some(resp) = resp {
        assert_eq!(
            resp.status(),
            413,
            "oversized block part must be rejected with 413"
        );
    }

    let data = b"hello";
    let block_id = {
        use sha1::{Digest, Sha1};
        let mut h = Sha1::new();
        h.update(data);
        hex::encode(h.finalize())
    };
    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(data.to_vec()).file_name(block_id.clone()),
    );
    let resp = f.client.post_multipart_url(&url, form).await;
    assert_eq!(resp.status(), 200, "small block upload should succeed");
}

// In-transit streaming (in-order chunks CDC'd into blocks) tests

/// Deterministic pseudo-random bytes (same generator as the `file_ops` unit
/// tests) so a fixture spanning several CDC blocks is reproducible.
fn pseudo_data(len: usize) -> Vec<u8> {
    let mut x: u64 = 0x9E3779B97F4A7C15;
    (0..len)
        .map(|_| {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (x >> 33) as u8
        })
        .collect()
}

/// Recursively count the block files under `dir` (the store lays them out as
/// `{xx}/{sha1}` under the block root).
fn count_files(dir: &std::path::Path) -> usize {
    fn walk(d: &std::path::Path, n: &mut usize) {
        if let Ok(entries) = std::fs::read_dir(d) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, n);
                } else {
                    *n += 1;
                }
            }
        }
    }
    let mut n = 0;
    walk(dir, &mut n);
    n
}

/// In-order chunks are CDC'd into the block store as they arrive (in-transit
/// streaming), so the final chunk commits without re-reading the temp file.
/// The committed file's block ids must equal whole-file CDC chunking.
#[tokio::test]
async fn test_chunked_upload_streams_in_order_to_blocks() {
    let f = TestFixture::new().await;
    let base = f.server.base_url.clone();
    let repo_id = f.repo_id.clone();

    let token = get_upload_token(&f).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();

    // 12 MiB deterministic pseudo-random content spanning several CDC blocks at
    // seafile's official block sizes (8 MiB avg / 6 MiB min / 10 MiB max) — the
    // same generator/size as the `file_ops` fixture that provably spans
    // multiple blocks.
    let content = pseudo_data(12 * 1024 * 1024 + 123);

    // Reference: whole-file CDC chunking → expected block ids (block id is the
    // SHA-1 of the block content, matching `write_block`).
    let mut chunker = infra::storage::cdc::Chunker::new(content.len());
    let mut expected_ids = Vec::new();
    for blk in chunker.feed(&content) {
        expected_ids.push(infra::crypto::fs_id::sha1_hex(&blk));
    }
    let tail = chunker.finish();
    if !tail.is_empty() {
        expected_ids.push(infra::crypto::fs_id::sha1_hex(&tail));
    }
    assert!(expected_ids.len() > 1, "fixture must span multiple blocks");

    let chunk_size = 8192usize;
    let n_chunks = content.len().div_ceil(chunk_size);
    for i in 0..n_chunks - 1 {
        let start = i * chunk_size;
        let end = ((i + 1) * chunk_size - 1).min(content.len() - 1);
        let resp = client
            .post(format!("{}/upload-aj/{}", base, token))
            .header(
                "content-range",
                format!("bytes {}-{}/{}", start, end, content.len()),
            )
            .multipart(chunked_upload_form(&repo_id, content[start..=end].to_vec()))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "chunk {i} failed");
    }

    // Blocks must already be persisted mid-upload: the in-transit streaming
    // path writes them as they complete, whereas the temp-file fallback would
    // write nothing until the final chunk.
    let port: u16 = f
        .server
        .base_url
        .rsplit(':')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let blocks_dir = std::env::temp_dir().join(format!("nf-test-{port}/blocks"));
    let block_files = count_files(&blocks_dir);
    assert!(
        block_files > 0,
        "expected in-transit blocks before the final chunk, found none"
    );

    let i = n_chunks - 1;
    let start = i * chunk_size;
    let end = content.len() - 1;
    let resp = client
        .post(format!("{}/upload-aj/{}", base, token))
        .header(
            "content-range",
            format!("bytes {}-{}/{}", start, end, content.len()),
        )
        .multipart(chunked_upload_form(&repo_id, content[start..=end].to_vec()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.is_array(), "final chunk should return the file list");
    assert_eq!(body[0]["size"], content.len() as i64);

    let resp = f
        .client
        .download_file(&f.api_token, &repo_id, "/big.txt")
        .await;
    assert_eq!(resp.status(), 200);
    let downloaded = resp.bytes().await.unwrap();
    assert_eq!(downloaded.as_ref(), content.as_slice());

    // Re-chunking the downloaded bytes must reproduce the reference ids,
    // proving the in-transit streaming produced exactly the blocks that
    // whole-file chunking (the previous final-assembly path) would.
    let mut chunker = infra::storage::cdc::Chunker::new(downloaded.len());
    let mut roundtrip_ids = Vec::new();
    for blk in chunker.feed(downloaded.as_ref()) {
        roundtrip_ids.push(infra::crypto::fs_id::sha1_hex(&blk));
    }
    let tail = chunker.finish();
    if !tail.is_empty() {
        roundtrip_ids.push(infra::crypto::fs_id::sha1_hex(&tail));
    }
    assert_eq!(roundtrip_ids, expected_ids);
}

/// Truncate the first content-addressed block file under `block_dir` (simulating
/// a partial write left by a killed writer) and return its remaining length;
/// `0` when no block exists yet.
fn truncate_one_block_file(block_dir: &std::path::Path) -> u64 {
    let mut stack = vec![block_dir.join("repos")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.len() == 40 && name.bytes().all(|b| b.is_ascii_hexdigit()) {
                let half = std::fs::metadata(&path).map(|m| m.len() / 2).unwrap_or(0);
                std::fs::write(&path, vec![0u8; half as usize]).unwrap();
                return half;
            }
        }
    }
    0
}

/// A block the in-transit streaming path already persisted must not be trusted
/// blindly by the fast commit: truncating it (a partial write) has to fall back
/// to the assembled temp file, whose block writes repair the stored block. The
/// committed file must still round-trip the exact bytes.
#[tokio::test]
async fn test_chunked_upload_repairs_a_truncated_streamed_block() {
    let f = TestFixture::new().await;
    let base = f.server.base_url.clone();
    let repo_id = f.repo_id.clone();
    let token = get_upload_token(&f).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();

    // 12 MiB spans several official CDC blocks, so the chunker has emitted
    // blocks well before the final chunk.
    let content = pseudo_data(12 * 1024 * 1024 + 123);
    let chunk_size = 8192usize;
    let n_chunks = content.len().div_ceil(chunk_size);
    for i in 0..n_chunks - 1 {
        let start = i * chunk_size;
        let end = ((i + 1) * chunk_size - 1).min(content.len() - 1);
        let resp = client
            .post(format!("{}/upload-aj/{}", base, token))
            .header(
                "content-range",
                format!("bytes {}-{}/{}", start, end, content.len()),
            )
            .multipart(chunked_upload_form(&repo_id, content[start..=end].to_vec()))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "chunk {i} failed");
    }

    let remaining = truncate_one_block_file(&f.server.block_dir);
    assert!(
        remaining > 0,
        "expected at least one streamed block on disk"
    );
    // The presence cache is per-process and would otherwise hold the length the
    // block had before it was truncated.
    f.server.state.block_store.invalidate_exists_cache();

    let i = n_chunks - 1;
    let start = i * chunk_size;
    let end = content.len() - 1;
    let resp = client
        .post(format!("{}/upload-aj/{}", base, token))
        .header(
            "content-range",
            format!("bytes {}-{}/{}", start, end, content.len()),
        )
        .multipart(chunked_upload_form(&repo_id, content[start..=end].to_vec()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "final chunk failed");

    let resp = f
        .client
        .download_file(&f.api_token, &repo_id, "/big.txt")
        .await;
    assert_eq!(resp.status(), 200);
    let downloaded = resp.bytes().await.unwrap();
    assert_eq!(
        downloaded.as_ref(),
        content.as_slice(),
        "a truncated streamed block must not become a committed file"
    );
}
