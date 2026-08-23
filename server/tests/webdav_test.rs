//! Integration tests for the WebDAV endpoint and per-library WebDAV keys.

mod common;

use common::{TestFixture, TestServer, create_test_repo, create_test_user};
use reqwest::Method;
use server::repository::member::CreateMemberParams;

// ── Helpers ───────────────────────────────────────────────────────────────

/// A plain HTTP client for WebDAV requests (Basic auth not handled by
/// `TestClient`).
fn http() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

fn dav_url(base: &str, repo_id: &str, path: &str) -> String {
    format!("{base}/dav/{repo_id}{path}")
}

/// Send a WebDAV request with HTTP Basic auth.
async fn dav(
    client: &reqwest::Client,
    method: &str,
    url: &str,
    email: &str,
    key: &str,
) -> reqwest::Response {
    let m = Method::from_bytes(method.as_bytes()).unwrap();
    client
        .request(m, url)
        .basic_auth(email, Some(key))
        .send()
        .await
        .unwrap()
}

/// Generate a WebDAV key for `token`'s user on `repo_id` and return it.
async fn gen_key(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    repo_id: &str,
    name: &str,
) -> String {
    let resp = client
        .post(format!("{base}/api2/repos/{repo_id}/webdav-keys/"))
        .bearer_auth(token)
        .json(&serde_json::json!({ "name": name }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "generate webdav key failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    body["key"].as_str().unwrap().to_string()
}

/// PUT a file over WebDAV.
async fn dav_put(
    client: &reqwest::Client,
    base: &str,
    repo_id: &str,
    path: &str,
    email: &str,
    key: &str,
    data: &[u8],
) -> reqwest::Response {
    let m = Method::from_bytes(b"PUT").unwrap();
    client
        .request(m, dav_url(base, repo_id, path))
        .basic_auth(email, Some(key))
        .body(data.to_vec())
        .send()
        .await
        .unwrap()
}

/// GET a file over WebDAV.
async fn dav_get(
    client: &reqwest::Client,
    base: &str,
    repo_id: &str,
    path: &str,
    email: &str,
    key: &str,
) -> reqwest::Response {
    dav(client, "GET", &dav_url(base, repo_id, path), email, key).await
}

// ── Key management API ────────────────────────────────────────────────────

#[tokio::test]
async fn test_key_generate_list_delete() {
    let f = TestFixture::new().await;
    let base = &f.server.base_url;
    let client = http();

    // Generate a key — plaintext returned exactly once.
    let resp = client
        .post(format!("{base}/api2/repos/{}/webdav-keys/", f.repo_id))
        .bearer_auth(&f.api_token)
        .json(&serde_json::json!({ "name": "MacBook" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let key = body["key"].as_str().unwrap().to_string();
    let key_id = body["key_id"].as_i64().unwrap();
    assert!(!key.is_empty());

    // List — no plaintext returned.
    let resp = client
        .get(format!("{base}/api2/repos/{}/webdav-keys/", f.repo_id))
        .bearer_auth(&f.api_token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["keys"].as_array().unwrap().len(), 1);
    assert!(body.to_string().contains(&key_id.to_string()));
    assert!(!body.to_string().contains(&key));

    // Delete the key.
    let resp = client
        .delete(format!(
            "{base}/api2/repos/{}/webdav-keys/{}/",
            f.repo_id, key_id
        ))
        .bearer_auth(&f.api_token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let resp = client
        .get(format!("{base}/api2/repos/{}/webdav-keys/", f.repo_id))
        .bearer_auth(&f.api_token)
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["keys"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_key_non_member_forbidden() {
    let f = TestFixture::new().await;
    // A second user who is NOT a member of the repo.
    create_test_user(f.server.db.as_ref(), "outsider@example.com", "password").await;
    let resp = f.client.login("outsider@example.com", "password").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let outsider_token = body["token"].as_str().unwrap().to_string();

    let resp = f
        .client
        .post_json(
            &format!("/api2/repos/{}/webdav-keys/", f.repo_id),
            Some(&outsider_token),
            &serde_json::json!({ "name": "x" }),
        )
        .await;
    assert_eq!(resp.status(), 403);
}

#[tokio::test]
async fn test_key_encrypted_repo_forbidden() {
    let f = TestFixture::new().await;
    let base = &f.server.base_url;
    let client = http();

    let resp = f
        .client
        .create_encrypted_repo_with_password(&f.api_token, "enc", "secret-pw")
        .await;
    assert_eq!(resp.status(), 201);
    let body: serde_json::Value = resp.json().await.unwrap();
    let enc_repo_id = body["id"].as_str().unwrap().to_string();

    // Generating a key for an encrypted repo is rejected.
    let resp = client
        .post(format!("{base}/api2/repos/{enc_repo_id}/webdav-keys/"))
        .bearer_auth(&f.api_token)
        .json(&serde_json::json!({ "name": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

// ── Authentication ────────────────────────────────────────────────────────

#[tokio::test]
async fn test_auth_no_key_unauthorized_with_challenge() {
    let f = TestFixture::new().await;
    let base = &f.server.base_url;
    let client = http();

    // No key exists yet → 401 with a Basic challenge.
    let resp = client
        .get(dav_url(base, &f.repo_id, "/"))
        .basic_auth(&f.email, Some("whatever"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
    let www = resp.headers().get("www-authenticate");
    assert!(www.is_some());
    assert!(www.unwrap().to_str().unwrap().contains("Basic"));
}

#[tokio::test]
async fn test_auth_wrong_key_unauthorized() {
    let f = TestFixture::new().await;
    let base = &f.server.base_url;
    let client = http();

    gen_key(&client, base, &f.api_token, &f.repo_id, "dev").await;
    let resp = dav(
        &client,
        "PROPFIND",
        &dav_url(base, &f.repo_id, "/"),
        &f.email,
        "definitely-not-the-key",
    )
    .await;
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_auth_nonexistent_repo_not_found() {
    let f = TestFixture::new().await;
    let base = &f.server.base_url;
    let client = http();

    let key = gen_key(&client, base, &f.api_token, &f.repo_id, "dev").await;
    let resp = dav(
        &client,
        "PROPFIND",
        &dav_url(base, "0000000000000000000000000000000000000000", "/"),
        &f.email,
        &key,
    )
    .await;
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn test_auth_non_member_unauthorized() {
    let f = TestFixture::new().await;
    let base = &f.server.base_url;
    let client = http();

    // A user who is not a member of the repo — auth fails even with a
    // well-formed key attempt because the membership check never passes.
    create_test_user(f.server.db.as_ref(), "outsider@example.com", "password").await;
    let resp = dav(
        &client,
        "PROPFIND",
        &dav_url(base, &f.repo_id, "/"),
        "outsider@example.com",
        "any-key",
    )
    .await;
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_auth_readonly_member_cannot_write() {
    let f = TestFixture::new().await;
    let base = &f.server.base_url;
    let client = http();

    // Member with read-only permission.
    let member_id = create_test_user(f.server.db.as_ref(), "reader@example.com", "password").await;
    f.server
        .repos
        .member
        .create_member(CreateMemberParams {
            repo_id: f.repo_id.clone(),
            user_id: member_id,
            permission: "r".to_string(),
            created_at: chrono::Utc::now().timestamp(),
        })
        .await
        .unwrap();

    let resp = f.client.login("reader@example.com", "password").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let reader_token = body["token"].as_str().unwrap().to_string();
    let reader_key = gen_key(&client, base, &reader_token, &f.repo_id, "reader").await;

    // Read works.
    let resp = dav(
        &client,
        "PROPFIND",
        &dav_url(base, &f.repo_id, "/"),
        "reader@example.com",
        &reader_key,
    )
    .await;
    assert_eq!(resp.status(), 207);

    // Write is forbidden.
    let resp = dav_put(
        &client,
        base,
        &f.repo_id,
        "/no.txt",
        "reader@example.com",
        &reader_key,
        b"nope",
    )
    .await;
    assert_eq!(resp.status(), 403);
}

#[tokio::test]
async fn test_webdav_globally_disabled() {
    let server = TestServer::start_with_webdav_enabled(false).await;
    let client = server.client();
    create_test_user(server.db.as_ref(), "t@example.com", "password").await;
    let resp = client.login("t@example.com", "password").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();
    let repo_id = create_test_repo(&client, &token, "repo").await;

    let http = http();
    let resp = http
        .get(dav_url(&server.base_url, &repo_id, "/"))
        .basic_auth("t@example.com", Some("any"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);
}

#[tokio::test]
async fn test_encrypted_repo_webdav_forbidden() {
    let f = TestFixture::new().await;
    let base = &f.server.base_url;
    let client = http();

    let resp = f
        .client
        .create_encrypted_repo_with_password(&f.api_token, "enc", "secret-pw")
        .await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let enc_repo_id = body["id"].as_str().unwrap().to_string();

    let resp = dav(
        &client,
        "PROPFIND",
        &dav_url(base, &enc_repo_id, "/"),
        &f.email,
        "some-key",
    )
    .await;
    assert_eq!(resp.status(), 403);
}

// ── File operations ───────────────────────────────────────────────────────

#[tokio::test]
async fn test_put_get_roundtrip() {
    let f = TestFixture::new().await;
    let base = &f.server.base_url;
    let client = http();
    let key = gen_key(&client, base, &f.api_token, &f.repo_id, "dev").await;

    let data = b"hello webdav";
    let resp = dav_put(
        &client,
        base,
        &f.repo_id,
        "/hello.txt",
        &f.email,
        &key,
        data,
    )
    .await;
    assert_eq!(resp.status(), 201);

    let resp = dav_get(&client, base, &f.repo_id, "/hello.txt", &f.email, &key).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.bytes().await.unwrap().as_ref(), data);
}

#[tokio::test]
async fn test_put_large_file_crosses_blocks() {
    let f = TestFixture::new().await;
    let base = &f.server.base_url;
    let client = http();
    let key = gen_key(&client, base, &f.api_token, &f.repo_id, "dev").await;

    // >4MiB of varied bytes so the streaming CDC chunker spans multiple
    // blocks — the WebDAV PUT path streams the body straight into block
    // writes instead of buffering the whole file in memory.
    let size = 4 * 1024 * 1024 + 123;
    let data: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
    let resp = dav_put(
        &client,
        base,
        &f.repo_id,
        "/large.bin",
        &f.email,
        &key,
        &data,
    )
    .await;
    assert_eq!(resp.status(), 201);

    let resp = dav_get(&client, base, &f.repo_id, "/large.bin", &f.email, &key).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.bytes().await.unwrap().as_ref(), data.as_slice());
}

#[tokio::test]
async fn test_put_overwrite_returns_204() {
    let f = TestFixture::new().await;
    let base = &f.server.base_url;
    let client = http();
    let key = gen_key(&client, base, &f.api_token, &f.repo_id, "dev").await;

    assert_eq!(
        dav_put(&client, base, &f.repo_id, "/a.txt", &f.email, &key, b"one")
            .await
            .status(),
        201
    );
    assert_eq!(
        dav_put(&client, base, &f.repo_id, "/a.txt", &f.email, &key, b"two")
            .await
            .status(),
        204
    );
    let resp = dav_get(&client, base, &f.repo_id, "/a.txt", &f.email, &key).await;
    assert_eq!(resp.bytes().await.unwrap().as_ref(), b"two");
}

#[tokio::test]
async fn test_put_missing_parent_conflict() {
    let f = TestFixture::new().await;
    let base = &f.server.base_url;
    let client = http();
    let key = gen_key(&client, base, &f.api_token, &f.repo_id, "dev").await;

    let resp = dav_put(
        &client,
        base,
        &f.repo_id,
        "/no/such/dir/file.txt",
        &f.email,
        &key,
        b"x",
    )
    .await;
    assert_eq!(resp.status(), 409);
}

#[tokio::test]
async fn test_mkcol_and_propfind() {
    let f = TestFixture::new().await;
    let base = &f.server.base_url;
    let client = http();
    let key = gen_key(&client, base, &f.api_token, &f.repo_id, "dev").await;

    let resp = dav(
        &client,
        "MKCOL",
        &dav_url(base, &f.repo_id, "/folder"),
        &f.email,
        &key,
    )
    .await;
    assert_eq!(resp.status(), 201);

    // Re-create → 405.
    let resp = dav(
        &client,
        "MKCOL",
        &dav_url(base, &f.repo_id, "/folder"),
        &f.email,
        &key,
    )
    .await;
    assert_eq!(resp.status(), 405);

    // Depth 1 listing shows the folder.
    let resp = dav(
        &client,
        "PROPFIND",
        &dav_url(base, &f.repo_id, "/"),
        &f.email,
        &key,
    )
    .await;
    assert_eq!(resp.status(), 207);
    let body = resp.text().await.unwrap();
    assert!(body.contains("D:multistatus"));
    assert!(body.contains("folder"));
}

#[tokio::test]
async fn test_delete_file_and_root() {
    let f = TestFixture::new().await;
    let base = &f.server.base_url;
    let client = http();
    let key = gen_key(&client, base, &f.api_token, &f.repo_id, "dev").await;

    dav_put(&client, base, &f.repo_id, "/d.txt", &f.email, &key, b"data").await;
    let resp = dav(
        &client,
        "DELETE",
        &dav_url(base, &f.repo_id, "/d.txt"),
        &f.email,
        &key,
    )
    .await;
    assert_eq!(resp.status(), 204);

    let resp = dav_get(&client, base, &f.repo_id, "/d.txt", &f.email, &key).await;
    assert_eq!(resp.status(), 404);

    // DELETE root → 405.
    let resp = dav(
        &client,
        "DELETE",
        &dav_url(base, &f.repo_id, "/"),
        &f.email,
        &key,
    )
    .await;
    assert_eq!(resp.status(), 405);
}

// ── MOVE / COPY ──────────────────────────────────────────────────────────

#[tokio::test]
async fn test_move_rename() {
    let f = TestFixture::new().await;
    let base = &f.server.base_url;
    let client = http();
    let key = gen_key(&client, base, &f.api_token, &f.repo_id, "dev").await;

    dav_put(&client, base, &f.repo_id, "/a.txt", &f.email, &key, b"data").await;
    let m = Method::from_bytes(b"MOVE").unwrap();
    let resp = client
        .request(m, dav_url(base, &f.repo_id, "/a.txt"))
        .basic_auth(&f.email, Some(&key))
        .header("Destination", dav_url(base, &f.repo_id, "/b.txt"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    assert_eq!(
        dav_get(&client, base, &f.repo_id, "/a.txt", &f.email, &key)
            .await
            .status(),
        404
    );
    assert_eq!(
        dav_get(&client, base, &f.repo_id, "/b.txt", &f.email, &key)
            .await
            .status(),
        200
    );
}

#[tokio::test]
async fn test_move_into_directory() {
    let f = TestFixture::new().await;
    let base = &f.server.base_url;
    let client = http();
    let key = gen_key(&client, base, &f.api_token, &f.repo_id, "dev").await;

    dav_put(&client, base, &f.repo_id, "/a.txt", &f.email, &key, b"data").await;
    dav(
        &client,
        "MKCOL",
        &dav_url(base, &f.repo_id, "/subdir"),
        &f.email,
        &key,
    )
    .await;
    let m = Method::from_bytes(b"MOVE").unwrap();
    let resp = client
        .request(m, dav_url(base, &f.repo_id, "/a.txt"))
        .basic_auth(&f.email, Some(&key))
        .header("Destination", dav_url(base, &f.repo_id, "/subdir/a.txt"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    assert_eq!(
        dav_get(&client, base, &f.repo_id, "/subdir/a.txt", &f.email, &key)
            .await
            .status(),
        200
    );
}

#[tokio::test]
async fn test_move_overwrite_precondition() {
    let f = TestFixture::new().await;
    let base = &f.server.base_url;
    let client = http();
    let key = gen_key(&client, base, &f.api_token, &f.repo_id, "dev").await;

    dav_put(&client, base, &f.repo_id, "/a.txt", &f.email, &key, b"a").await;
    dav_put(&client, base, &f.repo_id, "/b.txt", &f.email, &key, b"b").await;

    // Overwrite: F with existing destination → 412.
    let m = Method::from_bytes(b"MOVE").unwrap();
    let resp = client
        .request(m, dav_url(base, &f.repo_id, "/a.txt"))
        .basic_auth(&f.email, Some(&key))
        .header("Destination", dav_url(base, &f.repo_id, "/b.txt"))
        .header("Overwrite", "F")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 412);

    // Overwrite: T replaces the destination.
    let m = Method::from_bytes(b"MOVE").unwrap();
    let resp = client
        .request(m, dav_url(base, &f.repo_id, "/a.txt"))
        .basic_auth(&f.email, Some(&key))
        .header("Destination", dav_url(base, &f.repo_id, "/b.txt"))
        .header("Overwrite", "T")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);
    let resp = dav_get(&client, base, &f.repo_id, "/b.txt", &f.email, &key).await;
    assert_eq!(resp.bytes().await.unwrap().as_ref(), b"a");
}

#[tokio::test]
async fn test_copy_file() {
    let f = TestFixture::new().await;
    let base = &f.server.base_url;
    let client = http();
    let key = gen_key(&client, base, &f.api_token, &f.repo_id, "dev").await;

    dav_put(&client, base, &f.repo_id, "/a.txt", &f.email, &key, b"data").await;
    dav(
        &client,
        "MKCOL",
        &dav_url(base, &f.repo_id, "/subdir"),
        &f.email,
        &key,
    )
    .await;
    let m = Method::from_bytes(b"COPY").unwrap();
    let resp = client
        .request(m, dav_url(base, &f.repo_id, "/a.txt"))
        .basic_auth(&f.email, Some(&key))
        .header("Destination", dav_url(base, &f.repo_id, "/subdir/a.txt"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // Source still exists, copy exists.
    assert_eq!(
        dav_get(&client, base, &f.repo_id, "/a.txt", &f.email, &key)
            .await
            .status(),
        200
    );
    assert_eq!(
        dav_get(&client, base, &f.repo_id, "/subdir/a.txt", &f.email, &key)
            .await
            .status(),
        200
    );
}

#[tokio::test]
async fn test_copy_overwrite() {
    let f = TestFixture::new().await;
    let base = &f.server.base_url;
    let client = http();
    let key = gen_key(&client, base, &f.api_token, &f.repo_id, "dev").await;

    dav_put(&client, base, &f.repo_id, "/a.txt", &f.email, &key, b"new").await;
    dav(
        &client,
        "MKCOL",
        &dav_url(base, &f.repo_id, "/subdir"),
        &f.email,
        &key,
    )
    .await;
    dav_put(
        &client,
        base,
        &f.repo_id,
        "/subdir/a.txt",
        &f.email,
        &key,
        b"old",
    )
    .await;

    let m = Method::from_bytes(b"COPY").unwrap();
    let resp = client
        .request(m, dav_url(base, &f.repo_id, "/a.txt"))
        .basic_auth(&f.email, Some(&key))
        .header("Destination", dav_url(base, &f.repo_id, "/subdir/a.txt"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);
    let resp = dav_get(&client, base, &f.repo_id, "/subdir/a.txt", &f.email, &key).await;
    assert_eq!(resp.bytes().await.unwrap().as_ref(), b"new");
}

// ── Protocol details ──────────────────────────────────────────────────────

#[tokio::test]
async fn test_options_headers() {
    let f = TestFixture::new().await;
    let base = &f.server.base_url;
    let client = http();
    let key = gen_key(&client, base, &f.api_token, &f.repo_id, "dev").await;

    let resp = dav(
        &client,
        "OPTIONS",
        &dav_url(base, &f.repo_id, "/"),
        &f.email,
        &key,
    )
    .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers().get("dav").unwrap(), "1, 2");
    let allow = resp
        .headers()
        .get("allow")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    for m in ["PROPFIND", "MKCOL", "MOVE", "COPY", "LOCK", "UNLOCK"] {
        assert!(allow.contains(m), "Allow header missing {m}: {allow}");
    }
}

#[tokio::test]
async fn test_propfind_properties() {
    let f = TestFixture::new().await;
    let base = &f.server.base_url;
    let client = http();
    let key = gen_key(&client, base, &f.api_token, &f.repo_id, "dev").await;

    dav_put(
        &client,
        base,
        &f.repo_id,
        "/notes.txt",
        &f.email,
        &key,
        b"abc",
    )
    .await;
    let resp = dav(
        &client,
        "PROPFIND",
        &dav_url(base, &f.repo_id, "/"),
        &f.email,
        &key,
    )
    .await;
    assert_eq!(resp.status(), 207);
    let body = resp.text().await.unwrap();
    assert!(body.contains("getcontentlength"));
    assert!(body.contains("getlastmodified"));
    assert!(body.contains("resourcetype"));
    assert!(body.contains("notes.txt"));
    assert!(body.contains(">3<") || body.contains(">3</D:getcontentlength>"));
}

#[tokio::test]
async fn test_propfind_infinity() {
    let f = TestFixture::new().await;
    let base = &f.server.base_url;
    let client = http();
    let key = gen_key(&client, base, &f.api_token, &f.repo_id, "dev").await;

    dav_put(&client, base, &f.repo_id, "/top.txt", &f.email, &key, b"1").await;
    dav(
        &client,
        "MKCOL",
        &dav_url(base, &f.repo_id, "/dir"),
        &f.email,
        &key,
    )
    .await;
    dav_put(
        &client,
        base,
        &f.repo_id,
        "/dir/deep.txt",
        &f.email,
        &key,
        b"2",
    )
    .await;

    let resp = dav(
        &client,
        "PROPFIND",
        &dav_url(base, &f.repo_id, "/"),
        &f.email,
        &key,
    )
    .await;
    assert_eq!(resp.status(), 207);
    let body = resp.text().await.unwrap();
    assert!(body.contains("top.txt"));
    assert!(body.contains("deep.txt"));
}

#[tokio::test]
async fn test_propfind_bad_depth() {
    let f = TestFixture::new().await;
    let base = &f.server.base_url;
    let client = http();
    let key = gen_key(&client, base, &f.api_token, &f.repo_id, "dev").await;

    // Send an invalid Depth value via a raw request.
    let m = Method::from_bytes(b"PROPFIND").unwrap();
    let resp = client
        .request(m, dav_url(base, &f.repo_id, "/"))
        .basic_auth(&f.email, Some(&key))
        .header("Depth", "2")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn test_lock_unlock_proppatch() {
    let f = TestFixture::new().await;
    let base = &f.server.base_url;
    let client = http();
    let key = gen_key(&client, base, &f.api_token, &f.repo_id, "dev").await;

    let resp = dav(
        &client,
        "LOCK",
        &dav_url(base, &f.repo_id, "/"),
        &f.email,
        &key,
    )
    .await;
    assert_eq!(resp.status(), 200);
    assert!(resp.headers().get("lock-token").is_some());
    let body = resp.text().await.unwrap();
    assert!(body.contains("opaquelocktoken"));

    let resp = dav(
        &client,
        "UNLOCK",
        &dav_url(base, &f.repo_id, "/"),
        &f.email,
        &key,
    )
    .await;
    assert_eq!(resp.status(), 204);

    let resp = dav(
        &client,
        "PROPPATCH",
        &dav_url(base, &f.repo_id, "/"),
        &f.email,
        &key,
    )
    .await;
    assert_eq!(resp.status(), 207);
}

#[tokio::test]
async fn test_head() {
    let f = TestFixture::new().await;
    let base = &f.server.base_url;
    let client = http();
    let key = gen_key(&client, base, &f.api_token, &f.repo_id, "dev").await;

    dav_put(
        &client, base, &f.repo_id, "/h.txt", &f.email, &key, b"12345",
    )
    .await;
    let resp = dav(
        &client,
        "HEAD",
        &dav_url(base, &f.repo_id, "/h.txt"),
        &f.email,
        &key,
    )
    .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers().get("content-length").unwrap(), "5");
    assert!(resp.bytes().await.unwrap().is_empty());
}

#[tokio::test]
async fn test_special_characters() {
    let f = TestFixture::new().await;
    let base = &f.server.base_url;
    let client = http();
    let key = gen_key(&client, base, &f.api_token, &f.repo_id, "dev").await;

    let name = "测试 文档.txt";
    let encoded = percent_encoding::utf8_percent_encode(name, percent_encoding::NON_ALPHANUMERIC);
    let path = format!("/{encoded}");
    let data = "中文内容".as_bytes();
    let resp = dav_put(&client, base, &f.repo_id, &path, &f.email, &key, data).await;
    assert_eq!(resp.status(), 201, "PUT special-char file failed");

    let resp = dav_get(&client, base, &f.repo_id, &path, &f.email, &key).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.bytes().await.unwrap().as_ref(), data);
}

#[tokio::test]
async fn test_path_traversal_rejected() {
    let f = TestFixture::new().await;
    let base = &f.server.base_url;
    let client = http();
    let key = gen_key(&client, base, &f.api_token, &f.repo_id, "dev").await;

    let resp = dav_get(&client, base, &f.repo_id, "/../secret", &f.email, &key).await;
    // Rejected as bad request or not found — never succeeds.
    assert!(!resp.status().is_success());
}

#[tokio::test]
async fn test_delete_key_revokes_access() {
    let f = TestFixture::new().await;
    let base = &f.server.base_url;
    let client = http();

    // Generate a key and capture the plaintext + id.
    let resp = client
        .post(format!("{base}/api2/repos/{}/webdav-keys/", f.repo_id))
        .bearer_auth(&f.api_token)
        .json(&serde_json::json!({ "name": "dev" }))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let plaintext = body["key"].as_str().unwrap().to_string();
    let key_id = body["key_id"].as_i64().unwrap();

    // Works while present.
    assert_eq!(
        dav(
            &client,
            "PROPFIND",
            &dav_url(base, &f.repo_id, "/"),
            &f.email,
            &plaintext
        )
        .await
        .status(),
        207
    );

    // Delete → access revoked.
    let resp = client
        .delete(format!(
            "{base}/api2/repos/{}/webdav-keys/{key_id}/",
            f.repo_id
        ))
        .bearer_auth(&f.api_token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let resp = dav(
        &client,
        "PROPFIND",
        &dav_url(base, &f.repo_id, "/"),
        &f.email,
        &plaintext,
    )
    .await;
    assert_eq!(resp.status(), 401);
}
