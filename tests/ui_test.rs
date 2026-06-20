#![allow(dead_code)]

/// Web UI E2E tests — TDD approach.
mod common;

use common::{TestFixture, TestServer, create_test_user};

// ============================================================================
// Phase 1: Auth + Layout
// ============================================================================

fn no_redirect_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

#[tokio::test]
async fn test_login_page_returns_200() {
    let server = TestServer::start().await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/accounts/login/", server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "login page should return 200");

    let body = resp.text().await.unwrap();
    assert!(body.contains("Sign in"), "login page should contain form");
}

#[tokio::test]
async fn test_login_sets_session_cookie() {
    let server = TestServer::start().await;
    create_test_user(&server.db, "test@example.com", "password").await;

    let client = no_redirect_client();
    let resp = client
        .post(format!("{}/accounts/login/", server.base_url))
        .form(&[("email", "test@example.com"), ("password", "password")])
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 302, "login should redirect");

    let cookie = resp
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        cookie.contains("seahub-session="),
        "should set session cookie"
    );
    assert!(cookie.contains("HttpOnly"), "cookie should be HttpOnly");
    assert!(cookie.contains("Path=/"), "cookie path should be /");
}

#[tokio::test]
async fn test_login_invalid_password_returns_error() {
    let server = TestServer::start().await;
    create_test_user(&server.db, "test@example.com", "password").await;

    let resp = no_redirect_client()
        .post(format!("{}/accounts/login/", server.base_url))
        .form(&[("email", "test@example.com"), ("password", "wrongpass")])
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "failed login should return 200");
    let body = resp.text().await.unwrap();
    assert!(body.contains("Incorrect"), "should show error message");
}

#[tokio::test]
async fn test_login_nonexistent_user_returns_error() {
    let server = TestServer::start().await;

    let resp = no_redirect_client()
        .post(format!("{}/accounts/login/", server.base_url))
        .form(&[("email", "nobody@test.com"), ("password", "password")])
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "failed login should return 200");
    let body = resp.text().await.unwrap();
    assert!(body.contains("Incorrect"), "should show error message");
}

#[tokio::test]
async fn test_login_disabled_user_returns_error() {
    let server = TestServer::start().await;
    let db = &*server.db;
    use sea_orm::ActiveModelTrait;
    let user_id = create_test_user(db, "disabled@test.com", "password").await;
    let user = nanofile::entity::user::ActiveModel {
        id: sea_orm::Set(user_id),
        is_active: sea_orm::Set(false),
        ..Default::default()
    };
    user.update(db).await.unwrap();

    let resp = no_redirect_client()
        .post(format!("{}/accounts/login/", server.base_url))
        .form(&[("email", "disabled@test.com"), ("password", "password")])
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "disabled user login should return 200");
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("Incorrect email or password"),
        "disabled user should see generic error (no user enumeration): {body}"
    );
}

#[tokio::test]
async fn test_unauthenticated_access_redirects_to_login() {
    let server = TestServer::start().await;

    let resp = no_redirect_client()
        .get(format!("{}/libraries/", server.base_url))
        .send()
        .await
        .unwrap();

    // WebUser redirect uses Redirect::to() which returns 303
    assert!(
        resp.status() == 302 || resp.status() == 303,
        "unauthenticated should redirect, got: {}",
        resp.status()
    );
    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        location.contains("/accounts/login/"),
        "should redirect to /accounts/login/, got: {location}"
    );
}

#[tokio::test]
async fn test_logout_clears_session() {
    let fixture = TestFixture::new().await;

    let client = no_redirect_client();

    // Login
    let resp = client
        .post(format!("{}/accounts/login/", fixture.server.base_url))
        .form(&[("email", "test@example.com"), ("password", "password")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 302, "login should succeed");

    // Capture the session cookie from login response
    let session_cookie = resp
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    // Logout using the session cookie
    let logout = no_redirect_client()
        .get(format!("{}/accounts/logout/", fixture.server.base_url))
        .header("Cookie", &session_cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(logout.status(), 302, "logout should redirect");

    let set_cookie = logout
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        set_cookie.contains("seahub-session=;") || set_cookie.contains("Max-Age=0"),
        "should clear cookie, got: {set_cookie}"
    );
}

// ============================================================================
// Phase 2: Repos
// ============================================================================

/// Helper: login and return a client with session cookie stored
async fn login_client(fixture: &TestFixture) -> reqwest::Client {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .cookie_store(true)
        .build()
        .unwrap();

    let resp = client
        .post(format!("{}/accounts/login/", fixture.server.base_url))
        .form(&[("email", "test@example.com"), ("password", "password")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 302, "login for test helper");
    client // cookie stored by cookie_store
}

/// Extract CSRF token from any authenticated page that has a `data-csrf-token`
/// attribute. The caller must have a valid session cookie.
async fn get_csrf_token(client: &reqwest::Client, base_url: &str, repo_id: &str) -> String {
    let resp = client
        .get(format!("{}/library/{}/test-repo/", base_url, repo_id))
        .send()
        .await
        .unwrap();
    let html = resp.text().await.unwrap();
    html.split(r#"data-csrf-token=""#)
        .nth(1)
        .and_then(|s| s.split('"').next())
        .unwrap_or("")
        .to_string()
}

#[tokio::test]
async fn test_repo_list_page_shows_repos() {
    let fixture = TestFixture::new().await;
    let client = login_client(&fixture).await;

    let resp = client
        .get(format!("{}/libraries/", fixture.server.base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "repo list should return 200");
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("test-repo") || body.contains("Libraries"),
        "repo list should show repo name or at least render, got: {}",
        &body[..300.min(body.len())]
    );
}

#[tokio::test]
async fn test_repo_list_page_empty() {
    let fixture = TestFixture::no_repo("empty@test.com", "password").await;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .cookie_store(true)
        .build()
        .unwrap();
    let resp = client
        .post(format!("{}/accounts/login/", fixture.server.base_url))
        .form(&[("email", "empty@test.com"), ("password", "password")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 302, "login for empty user");

    let resp = client
        .get(format!("{}/libraries/", fixture.server.base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "empty repo list should return 200");
    let body = resp.text().await.unwrap();
    assert!(!body.is_empty(), "page should render even with no repos");
}

#[tokio::test]
async fn test_repo_detail_page_exists() {
    let fixture = TestFixture::new().await;
    let client = login_client(&fixture).await;

    let resp = client
        .get(format!(
            "{}/library/{}/test-repo/",
            fixture.server.base_url, fixture.repo_id
        ))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "repo detail should return 200");
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("test-repo") || body.contains("Libraries"),
        "repo detail should show repo info"
    );
}

// ============================================================================
// Phase 3: File Browser
// ============================================================================

#[tokio::test]
async fn test_file_list_shows_root_entries() {
    let fixture = TestFixture::new().await;
    let client = login_client(&fixture).await;

    // Upload a file first via API so it appears in the repo
    fixture
        .client
        .upload_file(
            &fixture.api_token,
            &fixture.repo_id,
            "/",
            "hello.txt",
            b"Hello, World!",
        )
        .await;

    // Now browse via UI
    let resp = client
        .get(format!(
            "{}/library/{}/test-repo/",
            fixture.server.base_url, fixture.repo_id
        ))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "file browser should return 200");
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("hello.txt"),
        "file list should show uploaded file, got: {}",
        &body[..300.min(body.len())]
    );
}

#[tokio::test]
async fn test_file_list_navigates_into_dir() {
    let fixture = TestFixture::new().await;

    // Create a directory and upload a file into it via API
    fixture
        .client
        .create_dir(&fixture.api_token, &fixture.repo_id, "/subdir")
        .await;
    fixture
        .client
        .upload_file(
            &fixture.api_token,
            &fixture.repo_id,
            "/subdir",
            "nested.txt",
            b"Nested content",
        )
        .await;

    let client = login_client(&fixture).await;

    // Browse into subdir via UI (Seahub path: /library/{id}/{name}/{path})
    let resp = client
        .get(format!(
            "{}/library/{}/test-repo/subdir",
            fixture.server.base_url, fixture.repo_id
        ))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "dir navigation should return 200");
    let body = resp.text().await.unwrap();
    assert!(body.contains("nested.txt"), "should show nested file");
}

#[tokio::test]
async fn test_file_list_empty_dir_shows_empty() {
    let fixture = TestFixture::new().await;
    fixture
        .client
        .create_dir(&fixture.api_token, &fixture.repo_id, "/emptydir")
        .await;

    let client = login_client(&fixture).await;
    let resp = client
        .get(format!(
            "{}/library/{}/test-repo/emptydir",
            fixture.server.base_url, fixture.repo_id
        ))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "empty dir should return 200");
    let body = resp.text().await.unwrap();
    assert!(!body.is_empty(), "should render content even for empty dir");
}

#[tokio::test]
async fn test_partial_list_returns_fragment() {
    let fixture = TestFixture::new().await;
    let client = login_client(&fixture).await;

    let resp = client
        .get(format!(
            "{}/library/{}/test-repo/?partial=1",
            fixture.server.base_url, fixture.repo_id
        ))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "partial list should return 200");
    let body = resp.text().await.unwrap();
    // Partial should NOT contain the full page layout
    assert!(
        !body.contains("</html>"),
        "partial response should be a fragment, not a full page"
    );
}

#[tokio::test]
async fn test_upload_file_creates_entry() {
    let fixture = TestFixture::new().await;
    let client = login_client(&fixture).await;

    // Upload via UI endpoint
    let file_bytes = b"UI upload test content";
    let file_part = reqwest::multipart::Part::bytes(file_bytes.to_vec())
        .file_name("ui_test.txt")
        .mime_str("text/plain")
        .unwrap();
    let form = reqwest::multipart::Form::new()
        .part("file", file_part)
        .text("parent_dir", "/");

    let resp = client
        .post(format!(
            "{}/library/{}/upload",
            fixture.server.base_url, fixture.repo_id
        ))
        .multipart(form)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 302, "upload should redirect");
    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        location.contains(&fixture.repo_id),
        "should redirect to repo"
    );

    // Verify file appears in listing
    let list_resp = client
        .get(format!(
            "{}/library/{}/test-repo/",
            fixture.server.base_url, fixture.repo_id
        ))
        .send()
        .await
        .unwrap();
    let body = list_resp.text().await.unwrap();
    assert!(
        body.contains("ui_test.txt"),
        "uploaded file should appear in listing"
    );
}

/// Simulate the frontend folder upload flow: upload files into nested
/// directories via the UI endpoint (/library/{id}/upload), which internally
/// calls `ensure_parent_dirs` + `create_file`. Verify that after uploading,
/// the file tree is correct — the uploaded folder does NOT contain root-level
/// files/folders, and root content remains intact.
#[tokio::test]
async fn test_folder_upload_does_not_leak_root_content() {
    let fixture = TestFixture::new().await;

    // ── Step 1: Add root-level content via API ──
    // Simulate a repo that already has files/folders at root.
    fixture
        .client
        .upload_file(
            &fixture.api_token,
            &fixture.repo_id,
            "/",
            "existing_root_file.txt",
            b"root level file",
        )
        .await;
    fixture
        .client
        .create_dir(&fixture.api_token, &fixture.repo_id, "/ExistingFolder")
        .await;
    fixture
        .client
        .upload_file(
            &fixture.api_token,
            &fixture.repo_id,
            "/ExistingFolder",
            "file_in_existing.txt",
            b"existing folder content",
        )
        .await;

    // ── Step 2: Login via UI (gets session cookie) ──
    let ui_client = login_client(&fixture).await;

    // ── Step 3: Simulate folder upload via the UI endpoint ──
    //
    // Frontend uploads files one at a time via the queue.
    // Each file sends a POST to /library/{id}/upload with:
    //   parent_dir = current_path + "/" + webkitRelativePath's dir part
    //   repo_name = name
    //   file = the actual file
    //   xhr = "1"  (returns JSON instead of redirect)
    //
    // Scenario: user uploads "UploadedFolder" containing:
    //   UploadedFolder/
    //     SubDir/
    //       nested_file.txt
    //     root_file_in_folder.txt

    // File 1: deepest nesting — should trigger ensure_parent_dirs to
    // create /UploadedFolder and /UploadedFolder/SubDir
    let file1_part = reqwest::multipart::Part::bytes(b"nested content".to_vec())
        .file_name("nested_file.txt")
        .mime_str("text/plain")
        .unwrap();
    let form1 = reqwest::multipart::Form::new()
        .part("file", file1_part)
        .text("parent_dir", "/UploadedFolder/SubDir")
        .text("repo_name", "test-repo")
        .text("xhr", "1");
    let resp1 = ui_client
        .post(format!(
            "{}/library/{}/upload",
            fixture.server.base_url, fixture.repo_id
        ))
        .multipart(form1)
        .send()
        .await
        .unwrap();
    assert!(
        resp1.status().is_success(),
        "nested file upload should succeed, got {}",
        resp1.status()
    );

    // File 2: file directly in the uploaded folder (no subdir)
    let file2_part = reqwest::multipart::Part::bytes(b"root in folder".to_vec())
        .file_name("root_file_in_folder.txt")
        .mime_str("text/plain")
        .unwrap();
    let form2 = reqwest::multipart::Form::new()
        .part("file", file2_part)
        .text("parent_dir", "/UploadedFolder")
        .text("repo_name", "test-repo")
        .text("xhr", "1");
    let resp2 = ui_client
        .post(format!(
            "{}/library/{}/upload",
            fixture.server.base_url, fixture.repo_id
        ))
        .multipart(form2)
        .send()
        .await
        .unwrap();
    assert!(
        resp2.status().is_success(),
        "folder root file upload should succeed, got {}",
        resp2.status()
    );

    // ── Step 4: Verify via API ──

    // 4a. Root directory must contain: existing_root_file.txt, ExistingFolder,
    //     and UploadedFolder — nothing else, nothing missing.
    let root_resp = fixture
        .client
        .list_dir(&fixture.api_token, &fixture.repo_id, "/")
        .await;
    assert_eq!(root_resp.status(), 200, "list root dir");
    let root_entries: Vec<serde_json::Value> = root_resp.json().await.unwrap();
    let root_names: Vec<&str> = root_entries
        .iter()
        .filter_map(|e| e["name"].as_str())
        .collect();

    assert!(
        root_names.contains(&"existing_root_file.txt"),
        "root should contain existing_root_file.txt, got {:?}",
        root_names
    );
    assert!(
        root_names.contains(&"ExistingFolder"),
        "root should contain ExistingFolder, got {:?}",
        root_names
    );
    assert!(
        root_names.contains(&"UploadedFolder"),
        "root should contain UploadedFolder, got {:?}",
        root_names
    );
    assert_eq!(
        root_names.len(),
        3,
        "root must have exactly 3 entries (no leaked content), got {:?}",
        root_names
    );

    // 4b. UploadedFolder must contain ONLY SubDir and root_file_in_folder.txt
    //     — NOT existing_root_file.txt or ExistingFolder.
    let folder_resp = fixture
        .client
        .list_dir(&fixture.api_token, &fixture.repo_id, "/UploadedFolder")
        .await;
    assert_eq!(folder_resp.status(), 200, "list UploadedFolder dir");
    let folder_entries: Vec<serde_json::Value> = folder_resp.json().await.unwrap();
    let folder_names: Vec<&str> = folder_entries
        .iter()
        .filter_map(|e| e["name"].as_str())
        .collect();

    assert!(
        folder_names.contains(&"SubDir"),
        "UploadedFolder should contain SubDir, got {:?}",
        folder_names
    );
    assert!(
        folder_names.contains(&"root_file_in_folder.txt"),
        "UploadedFolder should contain root_file_in_folder.txt, got {:?}",
        folder_names
    );
    assert!(
        !folder_names.contains(&"existing_root_file.txt"),
        "UploadedFolder MUST NOT leak existing_root_file.txt, got {:?}",
        folder_names
    );
    assert!(
        !folder_names.contains(&"ExistingFolder"),
        "UploadedFolder MUST NOT leak ExistingFolder, got {:?}",
        folder_names
    );
    assert_eq!(
        folder_names.len(),
        2,
        "UploadedFolder must have exactly 2 entries, got {:?}",
        folder_names
    );

    // 4c. ExistingFolder must still contain file_in_existing.txt
    //     (existing content must not be affected by folder upload).
    let existing_resp = fixture
        .client
        .list_dir(&fixture.api_token, &fixture.repo_id, "/ExistingFolder")
        .await;
    assert_eq!(existing_resp.status(), 200, "list ExistingFolder dir");
    let existing_entries: Vec<serde_json::Value> = existing_resp.json().await.unwrap();
    let existing_names: Vec<&str> = existing_entries
        .iter()
        .filter_map(|e| e["name"].as_str())
        .collect();
    assert!(
        existing_names.contains(&"file_in_existing.txt"),
        "ExistingFolder should contain file_in_existing.txt, got {:?}",
        existing_names
    );
}

#[tokio::test]
async fn test_download_file_returns_content() {
    let fixture = TestFixture::new().await;

    // Upload via API
    fixture
        .client
        .upload_file(
            &fixture.api_token,
            &fixture.repo_id,
            "/",
            "download.txt",
            b"Download test content",
        )
        .await;

    let client = login_client(&fixture).await;
    let resp = client
        .get(format!(
            "{}/library/{}/download/download.txt",
            fixture.server.base_url, fixture.repo_id
        ))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "download should return 200");
    let body = resp.text().await.unwrap();
    assert_eq!(body, "Download test content", "should return file content");
}

#[tokio::test]
async fn test_delete_file_removes_entry() {
    let fixture = TestFixture::new().await;

    // Upload via API
    fixture
        .client
        .upload_file(
            &fixture.api_token,
            &fixture.repo_id,
            "/",
            "delete_me.txt",
            b"To be deleted",
        )
        .await;

    let client = login_client(&fixture).await;
    let csrf_token = get_csrf_token(&client, &fixture.server.base_url, &fixture.repo_id).await;

    // Delete via UI
    let resp = client
        .post(format!(
            "{}/library/{}/delete",
            fixture.server.base_url, fixture.repo_id
        ))
        .form(&[("p", "/delete_me.txt"), ("csrf_token", &csrf_token)])
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 302, "delete should redirect");

    // Verify file is gone
    let list_resp = client
        .get(format!(
            "{}/library/{}/test-repo/",
            fixture.server.base_url, fixture.repo_id
        ))
        .send()
        .await
        .unwrap();
    let body = list_resp.text().await.unwrap();
    assert!(
        !body.contains("delete_me.txt"),
        "deleted file should not appear"
    );
}

#[tokio::test]
async fn test_create_directory_works() {
    let fixture = TestFixture::new().await;
    let client = login_client(&fixture).await;
    let csrf_token = get_csrf_token(&client, &fixture.server.base_url, &fixture.repo_id).await;

    let resp = client
        .post(format!(
            "{}/library/{}/new-dir",
            fixture.server.base_url, fixture.repo_id
        ))
        .form(&[("p", "/new_folder"), ("csrf_token", &csrf_token)])
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 302, "create dir should redirect");

    // Verify dir appears in listing
    let list_resp = client
        .get(format!(
            "{}/library/{}/test-repo/",
            fixture.server.base_url, fixture.repo_id
        ))
        .send()
        .await
        .unwrap();
    let body = list_resp.text().await.unwrap();
    assert!(
        body.contains("new_folder"),
        "new directory should appear in listing"
    );
}

#[tokio::test]
async fn test_rename_file_works() {
    let fixture = TestFixture::new().await;

    fixture
        .client
        .upload_file(
            &fixture.api_token,
            &fixture.repo_id,
            "/",
            "old_name.txt",
            b"Rename test",
        )
        .await;

    let client = login_client(&fixture).await;
    let csrf_token = get_csrf_token(&client, &fixture.server.base_url, &fixture.repo_id).await;

    let resp = client
        .post(format!(
            "{}/library/{}/rename",
            fixture.server.base_url, fixture.repo_id
        ))
        .form(&[
            ("p", "/old_name.txt"),
            ("new_name", "new_name.txt"),
            ("csrf_token", &csrf_token),
        ])
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 302, "rename should redirect");

    // Verify old name gone, new name present
    let list_resp = client
        .get(format!(
            "{}/library/{}/test-repo/",
            fixture.server.base_url, fixture.repo_id
        ))
        .send()
        .await
        .unwrap();
    let body = list_resp.text().await.unwrap();
    assert!(!body.contains("old_name.txt"), "old name should not appear");
    assert!(body.contains("new_name.txt"), "new name should appear");
}

#[tokio::test]
async fn test_file_preview_text() {
    let fixture = TestFixture::new().await;

    fixture
        .client
        .upload_file(
            &fixture.api_token,
            &fixture.repo_id,
            "/",
            "readme.md",
            b"# Readme\n\nHello world",
        )
        .await;

    let client = login_client(&fixture).await;
    let resp = client
        .get(format!(
            "{}/library/{}/preview/readme.md",
            fixture.server.base_url, fixture.repo_id
        ))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "preview should return 200");
    let body = resp.text().await.unwrap();
    assert!(body.contains("Hello world"), "should show file content");
}

// ============================================================================
// Phase 4: Shares
// ============================================================================

#[tokio::test]
async fn test_share_links_list_shares() {
    let fixture = TestFixture::new().await;
    let client = login_client(&fixture).await;
    let resp = client
        .get(format!("{}/share/", fixture.server.base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "shares page should return 200");
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("Shares") || body.contains("shared"),
        "should show shares page content"
    );
}

#[tokio::test]
async fn test_create_share_link_works() {
    let fixture = TestFixture::new().await;
    let client = login_client(&fixture).await;
    let csrf_token = get_csrf_token(&client, &fixture.server.base_url, &fixture.repo_id).await;

    let resp = client
        .post(format!("{}/share/create", fixture.server.base_url))
        .form(&[
            ("repo_id", fixture.repo_id.as_str()),
            ("path", "/"),
            ("type", "f"),
            ("csrf_token", &csrf_token),
        ])
        .send()
        .await
        .unwrap();

    // Should redirect after creation
    assert_eq!(resp.status(), 302, "create share should redirect");
}

#[tokio::test]
async fn test_delete_share_link_works() {
    let fixture = TestFixture::new().await;
    let client = login_client(&fixture).await;
    let csrf_token = get_csrf_token(&client, &fixture.server.base_url, &fixture.repo_id).await;

    // First create a share link via UI
    let create_resp = client
        .post(format!("{}/share/create", fixture.server.base_url))
        .form(&[
            ("repo_id", fixture.repo_id.as_str()),
            ("path", "/"),
            ("csrf_token", &csrf_token),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(create_resp.status(), 302, "create share via UI");

    // Verify the shares page renders
    let list_resp = client
        .get(format!("{}/share/", fixture.server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(list_resp.status(), 200, "shares page");
    let list_body = list_resp.text().await.unwrap();
    assert!(!list_body.is_empty(), "shares page should have content");
}

// ============================================================================
// Phase 5: Settings + 2FA
// ============================================================================

#[tokio::test]
async fn test_settings_page_shows_info() {
    let fixture = TestFixture::new().await;
    let client = login_client(&fixture).await;

    let resp = client
        .get(format!("{}/profile/", fixture.server.base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "settings page should return 200");
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("Setting") || body.contains("setting"),
        "settings page should render"
    );
}

#[tokio::test]
async fn test_change_password_works() {
    let fixture = TestFixture::new().await;
    let client = login_client(&fixture).await;

    let resp = client
        .post(format!("{}/profile/password", fixture.server.base_url))
        .form(&[("old_password", "password"), ("new_password", "newpass123")])
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 302, "password change should redirect");

    // Verify new password works via API
    let api_resp = fixture.client.login(&fixture.email, "newpass123").await;
    assert_eq!(api_resp.status(), 200, "new password should work");
}

#[tokio::test]
async fn test_change_password_wrong_old() {
    let fixture = TestFixture::new().await;
    let client = login_client(&fixture).await;

    let resp = client
        .post(format!("{}/profile/password", fixture.server.base_url))
        .form(&[
            ("old_password", "wrongpass"),
            ("new_password", "newpass123"),
        ])
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        200,
        "failed password change should re-render form"
    );
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("error") || body.contains("incorrect") || body.contains("Incorrect"),
        "should show error message"
    );
}

// ============================================================================
// Phase 6: Search
// ============================================================================

#[tokio::test]
async fn test_search_page_accessible() {
    let fixture = TestFixture::new().await;
    let client = login_client(&fixture).await;

    let resp = client
        .get(format!("{}/search?q=test", fixture.server.base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "search page should return 200");
    let body = resp.text().await.unwrap();
    assert!(!body.is_empty(), "search page should render");
    assert!(
        !body.contains("Search functionality coming soon"),
        "should not show placeholder message"
    );
}

#[tokio::test]
async fn test_search_no_results_message() {
    let fixture = TestFixture::new().await;
    let client = login_client(&fixture).await;

    // Upload a file first so the repo has content
    let resp = fixture
        .client
        .upload_file(
            &fixture.api_token,
            &fixture.repo_id,
            "/",
            "hello.txt",
            b"hello",
        )
        .await;
    assert_eq!(resp.status(), 200);

    // Search for something that doesn't exist
    let resp = client
        .get(format!(
            "{}/search?q=zzzzz_nonexistent",
            fixture.server.base_url
        ))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "empty search should return 200");
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("No results found"),
        "should show no results message"
    );
}

#[tokio::test]
async fn test_search_shows_results() {
    let fixture = TestFixture::new().await;
    let client = login_client(&fixture).await;

    // Upload a file via API
    let resp = fixture
        .client
        .upload_file(
            &fixture.api_token,
            &fixture.repo_id,
            "/",
            "hello.txt",
            b"hello world",
        )
        .await;
    assert_eq!(resp.status(), 200);

    // Search via UI
    let resp = client
        .get(format!("{}/search?q=hello", fixture.server.base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("hello.txt"),
        "search results should show filename"
    );
    assert!(
        body.contains("test-repo"),
        "search results should show repo name"
    );
    assert!(body.contains("1 found"), "search results should show count");
}

#[tokio::test]
async fn test_search_multiple_results_in_ui() {
    let fixture = TestFixture::new().await;
    let client = login_client(&fixture).await;

    // Upload multiple files
    for i in 0..3 {
        let name = format!("document-{}.txt", i);
        let resp = fixture
            .client
            .upload_file(&fixture.api_token, &fixture.repo_id, "/", &name, b"data")
            .await;
        assert_eq!(resp.status(), 200);
    }

    // Search via UI
    let resp = client
        .get(format!("{}/search?q=document", fixture.server.base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("document-0.txt"), "should show document-0");
    assert!(body.contains("document-1.txt"), "should show document-1");
    assert!(body.contains("document-2.txt"), "should show document-2");
    assert!(body.contains("3 found"), "should show count of 3");
}

#[tokio::test]
async fn test_search_dir_and_file() {
    let fixture = TestFixture::new().await;
    let client = login_client(&fixture).await;

    // Create a directory first
    let resp = fixture
        .client
        .create_dir(&fixture.api_token, &fixture.repo_id, "/myfolder")
        .await;
    assert_eq!(resp.status(), 200, "create dir should succeed");

    // Upload a file into it
    let resp = fixture
        .client
        .upload_file(
            &fixture.api_token,
            &fixture.repo_id,
            "/myfolder",
            "inner.txt",
            b"data",
        )
        .await;
    assert_eq!(resp.status(), 200);

    // Search for the directory name
    let resp = client
        .get(format!("{}/search?q=myfolder", fixture.server.base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("myfolder"),
        "search should show directory name"
    );

    // Search for the inner file name
    let resp = client
        .get(format!("{}/search?q=inner", fixture.server.base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("inner.txt"),
        "search should show inner file name"
    );
}

// ============================================================================
// Phase 7: Client Login
// ============================================================================

#[tokio::test]
async fn test_client_login_api_generates_token() {
    let fixture = TestFixture::new().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/api2/client-login/", fixture.server.base_url))
        .header("Authorization", format!("Token {}", fixture.api_token))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "client-login API should return 200");
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap_or("");
    assert_eq!(token.len(), 32, "token should be 32 chars");
}

#[tokio::test]
async fn test_client_login_api_requires_auth() {
    let server = TestServer::start().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/api2/client-login/", server.base_url))
        .send()
        .await
        .unwrap();

    // Without auth, should fail
    assert_ne!(resp.status(), 200, "should require auth");
}

#[tokio::test]
async fn test_client_login_flow_works() {
    let fixture = TestFixture::new().await;
    let api_client = reqwest::Client::new();

    // Step 1: Get token from API
    let resp = api_client
        .post(format!("{}/api2/client-login/", fixture.server.base_url))
        .header("Authorization", format!("Token {}", fixture.api_token))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    // Step 2: Use token to auto-login via browser endpoint
    let browser = no_redirect_client();
    let resp = browser
        .get(format!(
            "{}/client-login/?token={}&next=/libraries/",
            fixture.server.base_url, token
        ))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 302, "client-login should redirect");

    let cookie = resp
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        cookie.contains("seahub-session="),
        "should set session cookie"
    );

    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        location.contains("/libraries/"),
        "should redirect to next URL"
    );
}

#[tokio::test]
async fn test_client_login_expired_token_redirects() {
    let fixture = TestFixture::new().await;

    // Create an expired token directly in DB
    use sea_orm::EntityTrait;
    let now = chrono::Utc::now().timestamp();
    nanofile::entity::client_login_token::Entity::insert(
        nanofile::entity::client_login_token::ActiveModel {
            token: sea_orm::Set("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string()),
            username: sea_orm::Set("test@example.com".to_string()),
            created_at: sea_orm::Set(now - 60), // 60 seconds ago (past 30s TTL)
        },
    )
    .exec(fixture.server.db.as_ref())
    .await
    .unwrap();

    let browser = no_redirect_client();
    let resp = browser
        .get(format!(
            "{}/client-login/?token=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa&next=/libraries/",
            fixture.server.base_url
        ))
        .send()
        .await
        .unwrap();

    // Should redirect without setting cookie (token expired)
    assert!(
        resp.status() == 302 || resp.status() == 303,
        "expired token should redirect"
    );
    let cookie = resp
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        !cookie.contains("seahub-session="),
        "should NOT set session cookie for expired token"
    );
}

#[tokio::test]
async fn test_client_login_invalid_token_redirects() {
    let fixture = TestFixture::new().await;

    let browser = no_redirect_client();
    let resp = browser
        .get(format!(
            "{}/client-login/?token=nonexistenttoken1234567890abcdef&next=/libraries/",
            fixture.server.base_url
        ))
        .send()
        .await
        .unwrap();

    // Should redirect without setting cookie (token not found)
    assert!(
        resp.status() == 302 || resp.status() == 303,
        "invalid token should redirect"
    );
    let cookie = resp
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        !cookie.contains("seahub-session="),
        "should NOT set session cookie for invalid token"
    );
}

#[tokio::test]
async fn test_client_login_no_token_redirects() {
    let fixture = TestFixture::new().await;

    let browser = no_redirect_client();
    let resp = browser
        .get(format!("{}/client-login/", fixture.server.base_url))
        .send()
        .await
        .unwrap();

    // Should redirect without setting cookie (no token)
    assert!(
        resp.status() == 302 || resp.status() == 303,
        "no token should redirect"
    );
}
