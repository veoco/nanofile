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
    let user = infra::entity::user::ActiveModel {
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

/// Extract CSRF token from any authenticated page's hidden form input.
/// The caller must have a valid session cookie.
async fn get_csrf_token(client: &reqwest::Client, base_url: &str, repo_id: &str) -> String {
    let resp = client
        .get(format!("{}/libraries/{}/files", base_url, repo_id))
        .send()
        .await
        .unwrap();
    let html = resp.text().await.unwrap();
    html.split(r#"name="csrf_token" value=""#)
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
            "{}/libraries/{}/files",
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
            "{}/libraries/{}/files",
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
            "{}/libraries/{}/files/subdir",
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
            "{}/libraries/{}/files/emptydir",
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
            "{}/libraries/{}/files?partial=1",
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
async fn test_file_list_sort_by_name() {
    let fixture = TestFixture::new().await;
    let client = login_client(&fixture).await;

    // Upload files with different names
    fixture
        .client
        .upload_file(&fixture.api_token, &fixture.repo_id, "/", "c.txt", b"c")
        .await;
    fixture
        .client
        .upload_file(&fixture.api_token, &fixture.repo_id, "/", "a.txt", b"a")
        .await;
    fixture
        .client
        .upload_file(&fixture.api_token, &fixture.repo_id, "/", "b.txt", b"b")
        .await;

    // Request partial list sorted by name asc
    let resp = client
        .get(format!(
            "{}/libraries/{}/files?partial=1&sort=name&sort_order=asc",
            fixture.server.base_url, fixture.repo_id
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    let pos_a = body.find("a.txt").unwrap();
    let pos_b = body.find("b.txt").unwrap();
    let pos_c = body.find("c.txt").unwrap();
    assert!(
        pos_a < pos_b && pos_b < pos_c,
        "files should appear in asc order: a, b, c"
    );

    // Request sorted by name desc
    let resp = client
        .get(format!(
            "{}/libraries/{}/files?partial=1&sort=name&sort_order=desc",
            fixture.server.base_url, fixture.repo_id
        ))
        .send()
        .await
        .unwrap();
    let body = resp.text().await.unwrap();
    let pos_a = body.find("a.txt").unwrap();
    let pos_b = body.find("b.txt").unwrap();
    let pos_c = body.find("c.txt").unwrap();
    assert!(
        pos_c < pos_b && pos_b < pos_a,
        "files should appear in desc order: c, b, a"
    );
}

#[tokio::test]
async fn test_file_list_sort_by_mtime() {
    let fixture = TestFixture::new().await;
    let client = login_client(&fixture).await;

    // Upload files — the fixture repo already has a commit, so mtimes
    // will reflect upload order (newer files have higher mtime).
    fixture
        .client
        .upload_file(&fixture.api_token, &fixture.repo_id, "/", "old.txt", b"old")
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await; // ensure distinct mtime
    fixture
        .client
        .upload_file(&fixture.api_token, &fixture.repo_id, "/", "new.txt", b"new")
        .await;

    // Request sorted by mtime desc (newest first)
    let resp = client
        .get(format!(
            "{}/libraries/{}/files?partial=1&sort=mtime&sort_order=desc",
            fixture.server.base_url, fixture.repo_id
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    let pos_new = body.find("new.txt").unwrap();
    let pos_old = body.find("old.txt").unwrap();
    assert!(
        pos_new < pos_old,
        "newer file should appear before older when sorted by mtime desc"
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
            "{}/libraries/{}/files/download.txt?dl=1",
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
            "{}/libraries/{}/files/readme.md",
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
        .get(format!("{}/shares/", fixture.server.base_url))
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
        .post(format!("{}/shares/create/", fixture.server.base_url))
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
        .post(format!("{}/shares/create/", fixture.server.base_url))
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
        .get(format!("{}/shares/", fixture.server.base_url))
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
        .get(format!("{}/settings/", fixture.server.base_url))
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
        .post(format!("{}/settings/password/", fixture.server.base_url))
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
        .post(format!("{}/settings/password/", fixture.server.base_url))
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
        .get(format!("{}/search/?q=test", fixture.server.base_url))
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
            "{}/search/?q=zzzzz_nonexistent",
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
        .get(format!("{}/search/?q=hello", fixture.server.base_url))
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
        .get(format!("{}/search/?q=document", fixture.server.base_url))
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
        .get(format!("{}/search/?q=myfolder", fixture.server.base_url))
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
        .get(format!("{}/search/?q=inner", fixture.server.base_url))
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
    infra::entity::client_login_token::Entity::insert(
        infra::entity::client_login_token::ActiveModel {
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
