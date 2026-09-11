mod common;

use common::{TestFixture, TestServer, create_test_user};

#[tokio::test]
async fn test_create_repo() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    let resp = client.create_repo(token, "My Library").await;
    assert_eq!(resp.status(), 201);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["id"].as_str().is_some());
    assert_eq!(body["name"].as_str().unwrap(), "My Library");
}

/// `GET /api2/default-repo/` must return the user's earliest-owned repo (the
/// proxy for the virtual-drive default library). Created repos use second
/// resolution for `created_at`, so space the two creations out to pin the
/// ordering.
#[tokio::test]
async fn test_default_repo_returns_earliest_owned() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let api_token = body["token"].as_str().unwrap();

    let first_id = common::create_test_repo(&client, api_token, "First Library").await;
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let second_id = common::create_test_repo(&client, api_token, "Second Library").await;
    assert_ne!(first_id, second_id, "two distinct repos required");

    let resp = client.get("/api2/default-repo/", Some(api_token)).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["exists"], true, "user owns repos");
    assert_eq!(
        body["repo_id"].as_str().unwrap(),
        first_id,
        "default repo must be the earliest-created one"
    );
}

/// Security: a repo name containing script-breaking characters must be
/// rejected at creation — repo names are rendered into inline `<script>`
/// blocks, so `</script>` must never be persisted.
#[tokio::test]
async fn test_create_repo_rejects_script_in_name() {
    let server = TestServer::start().await;
    let client = server.client();
    create_test_user(server.db.as_ref(), "x@example.com", "password123").await;
    let resp = client.login("x@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    let resp = client
        .create_repo(token, "</script><script>alert(1)</script>")
        .await;
    assert_eq!(resp.status(), 400, "script-in-name repo must be rejected");
}

/// Legal repo names with punctuation (parentheses, apostrophes) still work.
#[tokio::test]
async fn test_create_repo_allows_punctuation() {
    let server = TestServer::start().await;
    let client = server.client();
    create_test_user(server.db.as_ref(), "y@example.com", "password123").await;
    let resp = client.login("y@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    let resp = client.create_repo(token, "My (Team) Docs").await;
    assert_eq!(resp.status(), 201);
}

#[tokio::test]
async fn test_list_repos() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    client.create_repo(token, "Lib1").await;
    client.create_repo(token, "Lib2").await;

    let resp = client.list_repos(token).await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let repos = body.as_array().unwrap();
    assert_eq!(repos.len(), 2);
}

#[tokio::test]
async fn test_get_repo() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    let resp = client.create_repo(token, "My Library").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let repo_id = body["id"].as_str().unwrap();

    let resp = client.get_repo(token, repo_id).await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"].as_str().unwrap(), "My Library");
}

#[tokio::test]
async fn test_download_info() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, token, "My Library").await;

    let resp = client.download_info(token, &repo_id).await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["token"].as_str().is_some());
    assert_eq!(body["repo_id"].as_str().unwrap(), repo_id);
}

/// Regression: download-info must return all fields required by seaf-cli
/// (email, repo_name, repo_version, salt, permission, encrypted, magic, random_key, enc_version)
#[tokio::test]
async fn test_download_info_fields_complete() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, token, "My Library").await;

    let resp = client.download_info(token, &repo_id).await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();

    // Required by seaf-cli (KeyError if missing)
    assert_eq!(body["email"].as_str().unwrap(), "test@example.com");
    assert_eq!(body["repo_name"].as_str().unwrap(), "My Library");
    assert_eq!(body["repo_id"].as_str().unwrap(), repo_id);
    assert!(body["token"].as_str().unwrap().len() >= 40);

    // Required by seaf-daemon clone (uses .get() with defaults, but must be present)
    assert_eq!(body["repo_version"].as_i64().unwrap(), 1);
    // salt: None → null in JSON
    assert!(body["salt"].is_null());
    assert_eq!(body["permission"].as_str().unwrap(), "rw");

    // Encryption-related fields
    assert_eq!(body["encrypted"].as_str().unwrap(), "false");
    assert_eq!(body["enc_version"].as_i64().unwrap(), 0);
    assert!(body["magic"].is_null());
    assert!(body["random_key"].is_null());

    // Relay fields (not used but should be present)
    assert!(body["relay_id"].is_null());
    assert!(body["relay_addr"].is_null());
    assert!(body["relay_port"].is_null());
}

/// Regression: trailing slashes on API routes
#[tokio::test]
async fn test_trailing_slash_routes() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    // All these should return 200 (not 404), matching Seafile client URLs
    let repo_id = common::create_test_repo(&client, token, "Trailing Test").await;

    // GET /api2/repos/ with trailing slash
    let resp = client.list_repos(token).await;
    assert_eq!(resp.status(), 200);

    // GET /api2/repos/{id}/ with trailing slash
    let resp = client.get_repo(token, &repo_id).await;
    assert_eq!(resp.status(), 200);

    // GET /api2/repos/{id}/download-info/ with trailing slash
    let resp = client.download_info(token, &repo_id).await;
    assert_eq!(resp.status(), 200);
}

/// Regression: create repo with trailing slash
#[tokio::test]
async fn test_create_repo_trailing_slash() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "ci@test.com", "ci123456").await;
    let resp = client.login("ci@test.com", "ci123456").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    // POST /api2/repos/ (with trailing slash) must return 201
    let resp = client.create_repo(token, "Test Repo").await;
    assert_eq!(resp.status(), 201);
}

#[tokio::test]
async fn test_delete_repo() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, token, "My Library").await;

    let resp = client.delete_repo(token, &repo_id).await;
    assert_eq!(resp.status(), 200);

    let resp = client.get_repo(token, &repo_id).await;
    assert_eq!(resp.status(), 404);
}

/// B.11.1 — POST /api2/repos/{repo_id}/?op=rename — rename repo.
#[tokio::test]
async fn test_rename_repo_success() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .rename_repo(&f.api_token, &f.repo_id, "NewName")
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body,
        serde_json::Value::String("success".to_string()),
        "rename must return the JSON string \"success\" (Android SupportResponseConverter)"
    );

    // Verify via GET.
    let resp = f.client.get_repo(&f.api_token, &f.repo_id).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "NewName");
}

/// Regression: rename repo via multipart POST (Android client format).
#[tokio::test]
async fn test_rename_repo_multipart_body() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .rename_repo_multipart(&f.api_token, &f.repo_id, "MultipartName")
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body,
        serde_json::Value::String("success".to_string()),
        "rename must return the JSON string \"success\" (Android SupportResponseConverter)"
    );

    // Verify via GET.
    let resp = f.client.get_repo(&f.api_token, &f.repo_id).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "MultipartName");
}

/// B.11.2 — Non-owner cannot rename a repo.
#[tokio::test]
async fn test_rename_repo_non_owner() {
    let f = TestFixture::new().await;

    // Create a second user.
    create_test_user(f.server.db.as_ref(), "other@test.com", "password").await;
    let resp = f.client.login("other@test.com", "password").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let other_token = body["token"].as_str().unwrap();

    let resp = f
        .client
        .rename_repo(other_token, &f.repo_id, "Hacked")
        .await;
    assert_eq!(resp.status(), 403);
}

/// B.11.3 — Invalid name returns 400.
#[tokio::test]
async fn test_rename_repo_invalid_name() {
    let f = TestFixture::new().await;

    // Empty name.
    let resp = f.client.rename_repo(&f.api_token, &f.repo_id, "").await;
    assert_eq!(resp.status(), 400);

    // Name with slash.
    let resp = f
        .client
        .rename_repo(&f.api_token, &f.repo_id, "bad/name")
        .await;
    assert_eq!(resp.status(), 400);
}

/// B.11.4 — Non-owner cannot delete a repo.
#[tokio::test]
async fn test_delete_repo_non_owner() {
    let f = TestFixture::new().await;

    // Create a second user.
    create_test_user(f.server.db.as_ref(), "other2@test.com", "password").await;
    let resp = f.client.login("other2@test.com", "password").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let other_token = body["token"].as_str().unwrap();

    let resp = f.client.delete_repo(other_token, &f.repo_id).await;
    assert_eq!(resp.status(), 403);
}

/// B.11.5 — DELETE /api/v2.1/repos/{repo_id}/ — delete repo via v2.1 API.
#[tokio::test]
async fn test_delete_repo_v21_success() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .delete(
            &format!("/api/v2.1/repos/{}/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);

    // Verify it's gone.
    let resp = f.client.get_repo(&f.api_token, &f.repo_id).await;
    assert_eq!(resp.status(), 404);
}

/// B.11.6 — Non-owner cannot delete a repo via v2.1.
#[tokio::test]
async fn test_delete_repo_v21_non_owner() {
    let f = TestFixture::new().await;

    // Create a second user.
    create_test_user(f.server.db.as_ref(), "other3@test.com", "password").await;
    let resp = f.client.login("other3@test.com", "password").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let other_token = body["token"].as_str().unwrap();

    let resp = f
        .client
        .delete(
            &format!("/api/v2.1/repos/{}/", f.repo_id),
            Some(other_token),
        )
        .await;
    assert_eq!(resp.status(), 403);
}

/// B.11.7 — POST /api2/repos/ accepts JSON body (web frontend format).
#[tokio::test]
async fn test_create_repo_json_body() {
    let f = TestFixture::new().await;

    // Create a repo with JSON body (as the web frontend does).
    let resp = f
        .client
        .post_json(
            "/api2/repos/",
            Some(&f.api_token),
            &serde_json::json!({"name": "JSON Created Repo"}),
        )
        .await;
    assert_eq!(resp.status(), 201);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "JSON Created Repo");
    assert!(body["id"].as_str().is_some());
}

/// Regression: POST /api2/repos/ accepts multipart body (Android client format).
#[tokio::test]
async fn test_create_repo_multipart_body() {
    let f = TestFixture::new().await;

    // Create a repo with multipart body (as the Android client does).
    let resp = f
        .client
        .create_repo_multipart(&f.api_token, "Multipart Created Repo")
        .await;
    assert_eq!(resp.status(), 201);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "Multipart Created Repo");
    assert_eq!(body["repo_name"], "Multipart Created Repo");
    assert!(body["id"].as_str().is_some());
    assert!(body["repo_id"].as_str().is_some());
    assert!(body["token"].as_str().is_some());
}

/// Regression: POST /api2/repos/ accepts multipart body with description.
#[tokio::test]
async fn test_create_repo_multipart_with_desc() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .create_repo_multipart_with_desc(&f.api_token, "Repo With Desc", "A test repo")
        .await;
    assert_eq!(resp.status(), 201);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "Repo With Desc");
    assert_eq!(body["repo_name"], "Repo With Desc");
    assert_eq!(body["desc"], "A test repo");
    assert!(body["id"].as_str().is_some());
}

// ============================================================================
// Security: repo-scoped metadata / thumbnail / exif / history require membership
// ============================================================================

/// Security: a non-member must not read another user's repo metadata,
/// thumbnails, EXIF data, or history.
#[tokio::test]
async fn test_metadata_thumbnail_exif_history_require_membership() {
    let f = TestFixture::new().await;

    // Second user who is not a member of f.repo_id.
    create_test_user(f.server.db.as_ref(), "other@example.com", "password123").await;
    let resp = f.client.login("other@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let b_token = body["token"].as_str().unwrap().to_string();

    let rid = &f.repo_id;

    // Metadata config
    let resp = f
        .client
        .get(&format!("/api/v2.1/repos/{rid}/metadata/"), Some(&b_token))
        .await;
    assert_eq!(
        resp.status(),
        403,
        "metadata config must require membership"
    );

    // Metadata tags
    let resp = f
        .client
        .get(
            &format!("/api/v2.1/repos/{rid}/metadata/tags/"),
            Some(&b_token),
        )
        .await;
    assert_eq!(resp.status(), 403, "metadata tags must require membership");

    // Thumbnail
    let resp = f
        .client
        .get(
            &format!("/api2/repos/{rid}/thumbnail/?p=/test.txt&size=48"),
            Some(&b_token),
        )
        .await;
    assert_eq!(resp.status(), 403, "thumbnail must require membership");

    // EXIF
    let resp = f
        .client
        .get(
            &format!("/api2/repos/{rid}/file/exif/?p=/test.txt"),
            Some(&b_token),
        )
        .await;
    assert_eq!(resp.status(), 403, "exif must require membership");

    // History
    let resp = f
        .client
        .get(
            &format!("/api2/repo_history_changes/{rid}/?commit_id=0000000000000000000000000000000000000000"),
            Some(&b_token),
        )
        .await;
    assert_eq!(resp.status(), 403, "history must require membership");

    // Owner still has access.
    let resp = f
        .client
        .get(
            &format!("/api/v2.1/repos/{rid}/metadata/"),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200, "owner keeps access to metadata");
}

// ── repo_id validation ────────────────────────────────────────────────

/// POST /api2/repos/ with an explicit `repo_id` in the body.
async fn create_repo_with_id(f: &TestFixture, name: &str, repo_id: &str) -> reqwest::Response {
    f.client
        .post_json(
            "/api2/repos/",
            Some(&f.api_token),
            &serde_json::json!({ "name": name, "repo_id": repo_id }),
        )
        .await
}

/// Regression: `repo_id` is interpolated into on-disk directory names
/// (temp uploads, thumbnail cache), so a client-supplied value must be a
/// well-formed UUID. Free-form ids such as `../../x` or `/etc/cron.d/y` used to
/// be accepted and let an authenticated user create directories and write
/// files outside the storage root.
#[tokio::test]
async fn test_create_repo_rejects_non_uuid_repo_id() {
    let f = TestFixture::new().await;

    for bad_id in [
        "../../nf-traversal-poc",
        "../../../../../../tmp/nf-traversal-poc",
        "/etc/cron.d/nanofile-poc",
        "not-a-uuid",
        "..",
        "cfcab3e0-9eb4-4c4f-92d0-87db2cd8290", // 35 chars: one short
        "cfcab3e0-9eb4-4c4f-92d0-87db2cd8290dd", // 37 chars: one long
        "cfcab3e0_9eb4_4c4f_92d0_87db2cd8290d", // wrong separator
        "cfcab3e0-9eb4-4c4f-92d0-87db2cd8290g", // non-hex char
    ] {
        let resp = create_repo_with_id(&f, "trav", bad_id).await;
        assert_eq!(
            resp.status(),
            400,
            "repo_id {bad_id:?} must be rejected, body={:?}",
            resp.text().await
        );
    }

    // None of the rejected ids may exist in the database.
    use sea_orm::EntityTrait;
    let count = infra::entity::repo::Entity::find()
        .all(f.server.db.as_ref())
        .await
        .unwrap();
    assert!(
        count.iter().all(|r| uuid::Uuid::parse_str(&r.id).is_ok()),
        "every stored repo id must be a valid UUID, got {:?}",
        count.iter().map(|r| r.id.clone()).collect::<Vec<_>>()
    );
}

/// A client-proposed UUID is still accepted (the sync clients generate their
/// own ids) and is normalised to canonical lowercase form.
#[tokio::test]
async fn test_create_repo_accepts_valid_uuid_repo_id() {
    let f = TestFixture::new().await;

    let proposed = "A1B2C3D4-E5F6-4A7B-8C9D-0E1F2A3B4C5D";
    let resp = create_repo_with_id(&f, "client-id-repo", proposed).await;
    assert_eq!(resp.status(), 201, "a valid UUID must be accepted");

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["repo_id"].as_str().unwrap(),
        proposed.to_lowercase(),
        "the id must be normalised to canonical form"
    );
}

/// Regression: temp uploads are stored under `{temp_dir}/upload/<hash>`,
/// never under a directory named after the raw repo id, so the identifier can
/// no longer address a path outside the storage root.
#[tokio::test]
async fn test_chunked_upload_temp_dir_is_hashed_not_raw_repo_id() {
    let f = TestFixture::new().await;
    let base = f.server.base_url.clone();
    let repo_id = f.repo_id.clone();

    // Mint a short-lived upload token (`GET /api2/repos/{id}/upload-link/`),
    // then send an intermediate Content-Range chunk. Intermediate chunks are
    // buffered to a temp file instead of being committed.
    let resp = f
        .client
        .get(
            &format!("/api2/repos/{repo_id}/upload-link/?from=web"),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200, "upload-link request failed");
    let url: String = resp.json().await.unwrap();
    let upload_token = url
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap()
        .to_string();

    let content = b"hello world!";
    let split = 6;
    let (c1, _) = content.split_at(split);
    let form = reqwest::multipart::Form::new()
        .part(
            "file",
            reqwest::multipart::Part::bytes(c1.to_vec()).file_name("big.txt"),
        )
        .text("repo_id", repo_id.clone())
        .text("parent_dir", "/")
        .text("relative_path", "");

    let resp = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!("{base}/upload-aj/{upload_token}"))
        .header(
            "content-range",
            format!("bytes 0-{}/{}", split - 1, content.len()),
        )
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "intermediate chunk should be accepted");

    let port = base.rsplit(':').next().unwrap().to_string();
    let upload_root = std::env::temp_dir()
        .join(format!("nf-test-{port}/tmp"))
        .join("upload");

    let hashed = infra::crypto::fs_id::sha1_hex(repo_id.as_bytes());
    assert!(
        upload_root.join(&hashed).is_dir(),
        "temp uploads must live in a hashed repo dir ({})",
        upload_root.join(&hashed).display()
    );
    assert!(
        !upload_root.join(&repo_id).exists(),
        "no directory may be named after the raw repo id"
    );

    // Nothing escaped `{temp_dir}/upload/`.
    for entry in std::fs::read_dir(&upload_root).unwrap() {
        let path = entry.unwrap().path();
        assert!(
            path.starts_with(&upload_root),
            "temp upload dir escaped upload root: {}",
            path.display()
        );
        assert_ne!(
            path.file_name().unwrap().to_string_lossy(),
            repo_id,
            "a temp dir must never be named after the raw repo id"
        );
    }
}

// ── 官方客户端兼容性（模拟客户端真实请求）──────────────────────────────────

/// Compatibility: the official desktop client (`src/ui/create-repo-dialog.cpp`)
/// creates a **plain** library without any `repo_id`, letting the server
/// generate one (`CreateRepoRequest(account_, name_, name_, passwd_)`).
/// The validation must not disturb that path.
#[tokio::test]
async fn test_compat_desktop_plain_repo_without_repo_id() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .post_json(
            "/api2/repos/",
            Some(&f.api_token),
            &serde_json::json!({ "name": "desktop-plain", "desc": "desktop-plain" }),
        )
        .await;
    assert_eq!(resp.status(), 201, "plain create without repo_id must work");

    let body: serde_json::Value = resp.json().await.unwrap();
    let repo_id = body["repo_id"].as_str().expect("repo_id in response");
    assert!(
        uuid::Uuid::parse_str(repo_id).is_ok(),
        "server-generated id must be a UUID, got {repo_id}"
    );

    // The client then syncs that id; the sync token flow must accept it.
    let token = common::get_sync_token(&f.client, &f.api_token, repo_id).await;
    let resp = f.client.get_head_commit(&token, repo_id).await;
    assert_eq!(resp.status(), 200, "sync on the new repo must work");
}

/// Compatibility: the desktop client's **encrypted-library creation** payload,
/// reproduced exactly.
///
/// `CreateRepoRequest` in `seafile-client/src/api/requests.cpp` sends
/// `application/x-www-form-urlencoded` with every value stringified by
/// `QString::number()` / `setFormParam`:
///
/// ```text
/// name, desc, enc_version="4", repo_id, magic, random_key, salt,
/// pwd_hash_algo, pwd_hash_params, pwd_hash
/// ```
///
/// Note there is **no** `encrypted` field — the client only ever sent that
/// through `passwd`-based creation. Two things used to break this request:
/// `enc_version` typed as a number (deserialization failed → 500 for *every*
/// encrypted create), and the encryption material being ignored (the library
/// was stored as plaintext, so the client's own key derivation no longer
/// matched).
#[tokio::test]
async fn test_compat_desktop_encrypted_create_form_payload() {
    let f = TestFixture::new().await;
    let password = "desktop-password";
    let repo_id = "3f2504e0-4f89-41d3-9a0c-0305e82c3301";
    let salt = "c".repeat(64);

    let magic = infra::crypto::key_derivation::generate_magic(repo_id, password, 4, &salt).unwrap();
    let random_key =
        infra::crypto::key_derivation::generate_random_key_for_repo(password, 4, &salt).unwrap();

    // Exactly the desktop's form fields, in the client's order, all as strings.
    let resp = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!("{}/api2/repos/", f.server.base_url))
        .bearer_auth(&f.api_token)
        .form(&[
            ("name", "desktop-encrypted"),
            ("desc", "desktop-encrypted"),
            ("enc_version", "4"),
            ("repo_id", repo_id),
            ("magic", magic.as_str()),
            ("random_key", random_key.as_str()),
            ("salt", salt.as_str()),
            ("pwd_hash_algo", "PBKDF2"),
            ("pwd_hash_params", "iterations=1000"),
            ("pwd_hash", "d".repeat(64).as_str()),
        ])
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        201,
        "desktop encrypted-create form payload must succeed, body={:?}",
        resp.text().await
    );

    let created: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(created["repo_id"].as_str().unwrap(), repo_id);
    assert_eq!(
        created["encrypted"], true,
        "the library must be stored encrypted even though the client sends no `encrypted` flag"
    );
    assert_eq!(created["enc_version"], 4);
    assert_eq!(created["magic"], magic);
    assert_eq!(created["random_key"], random_key);
    assert_eq!(
        created["salt"], salt,
        "the v4 per-library salt must be kept"
    );

    // The stored library must be usable: the client's password verifies
    // against it (i.e. magic/salt were stored, not discarded).
    let resp = f
        .client
        .set_repo_password_v2(&f.api_token, repo_id, password)
        .await;
    assert_eq!(
        resp.status(),
        200,
        "the client's password must verify against the created library"
    );
}

/// Compatibility: `enc_version` arrives as a string from the official clients,
/// so a malformed value must be a client error (400), never a 500.
#[tokio::test]
async fn test_compat_bad_enc_version_is_a_client_error() {
    let f = TestFixture::new().await;

    let resp = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!("{}/api2/repos/", f.server.base_url))
        .bearer_auth(&f.api_token)
        .form(&[("name", "bad-enc-version"), ("enc_version", "not-a-number")])
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        400,
        "a non-numeric enc_version must be a 400, not an internal error"
    );
}

/// Compatibility: the same failure mode through the JSON branch must also be a
/// 400 — `From<serde_json::Error>` used to turn it into a 500.
#[tokio::test]
async fn test_compat_malformed_json_create_is_a_client_error() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .post_json(
            "/api2/repos/",
            Some(&f.api_token),
            &serde_json::json!({ "name": "x", "enc_version": "not-a-number" }),
        )
        .await;
    assert_eq!(resp.status(), 400, "malformed JSON must be a 400");
}

/// Compatibility: a client-proposed UUID passes validation (and a malformed id
/// is still rejected) — the guarantee on the encrypted path.
#[tokio::test]
async fn test_compat_client_uuid_is_validated_not_rejected_wholesale() {
    let f = TestFixture::new().await;

    let client_repo_id = "3f2504e0-4f89-41d3-9a0c-0305e82c3302";
    let body = serde_json::json!({
        "name": "client-uuid-check",
        "enc_version": 2,
        "repo_id": client_repo_id,
        "magic": "a".repeat(64),
        "random_key": "b".repeat(96),
    });

    let resp = f
        .client
        .post_json("/api2/repos/", Some(&f.api_token), &body)
        .await;
    assert_eq!(
        resp.status(),
        201,
        "a client-proposed lowercase UUID must pass validation and create the library"
    );
    let created: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(created["repo_id"].as_str().unwrap(), client_repo_id);

    // The same request with a traversal id stays rejected — that is the fix.
    let mut bad = body.clone();
    bad["repo_id"] = serde_json::json!("../../client-uuid-check");
    bad["name"] = serde_json::json!("client-uuid-check-2");
    let resp = f
        .client
        .post_json("/api2/repos/", Some(&f.api_token), &bad)
        .await;
    assert_eq!(
        resp.status(),
        400,
        "a malformed id must be rejected even on the encrypted path"
    );
}

/// Compatibility: a client that derives keys from an **uppercase** UUID must
/// still end up with a working library. The provider stores the canonical
/// lowercase form, so the sync URL and the token binding agree.
#[tokio::test]
async fn test_compat_uppercase_client_uuid_round_trips_consistently() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .post_json(
            "/api2/repos/",
            Some(&f.api_token),
            &serde_json::json!({
                "name": "upper",
                "repo_id": "3F2504E0-4F89-41D3-9A0C-0305E82C3301",
            }),
        )
        .await;
    assert_eq!(resp.status(), 201);

    let body: serde_json::Value = resp.json().await.unwrap();
    let repo_id = body["repo_id"].as_str().unwrap().to_string();
    assert_eq!(repo_id, repo_id.to_lowercase());

    // A sync token is issued for the stored (lowercase) id and the sync URL
    // built from that id must authenticate.
    let token = common::get_sync_token(&f.client, &f.api_token, &repo_id).await;
    assert_eq!(
        f.client.get_head_commit(&token, &repo_id).await.status(),
        200
    );
}
