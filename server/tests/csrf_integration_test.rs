mod common;

use common::{TestServer, create_test_user};

/// Login via the Web UI (form POST) and return a reqwest client with
/// cookies stored (both `seahub-session` and `sfcsrftoken`).
async fn ui_login(server: &TestServer) -> reqwest::Client {
    let client = reqwest::Client::builder()
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let resp = client
        .post(format!("{}/accounts/login/", server.base_url))
        .form(&[("email", "test@example.com"), ("password", "password")])
        .send()
        .await
        .unwrap();
    assert!(
        resp.status() == 302,
        "login should redirect, got: {}",
        resp.status()
    );

    // Trigger a GET to store cookies from Set-Cookie headers.
    let _ = client
        .get(format!("{}/libraries/", server.base_url))
        .send()
        .await;

    client
}

/// Create an API token by calling the login endpoint as a client would.
async fn get_api_token(server: &TestServer) -> String {
    let resp = reqwest::Client::new()
        .post(format!("{}/api2/auth-token/", server.base_url))
        .form(&[("username", "test@example.com"), ("password", "password")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "API login should succeed");
    let body: serde_json::Value = resp.json().await.unwrap();
    body["token"].as_str().unwrap().to_string()
}

/// Create a repo and return its ID.
async fn create_repo_api(base_url: &str, api_token: &str, name: &str) -> String {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/api2/repos/", base_url))
        .header("Authorization", format!("Token {}", api_token))
        .form(&serde_json::json!({"name": name}))
        .send()
        .await
        .unwrap();
    assert!(
        resp.status() == 200 || resp.status() == 201,
        "create repo {name} returned {}",
        resp.status()
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    body["repo_id"].as_str().unwrap().to_string()
}

// ═══════════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════════

/// Login via UI sets both `seahub-session` (HttpOnly) and `sfcsrftoken` (non-HttpOnly).
#[tokio::test]
async fn test_login_sets_csrf_cookie() {
    let server = TestServer::start().await;
    create_test_user(server.db.as_ref(), "test@example.com", "password").await;

    let client = reqwest::Client::builder()
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let login_resp = client
        .post(format!("{}/accounts/login/", server.base_url))
        .form(&[("email", "test@example.com"), ("password", "password")])
        .send()
        .await
        .unwrap();
    assert_eq!(login_resp.status(), 302, "login should redirect");

    // Collect all Set-Cookie headers from the response.
    let set_cookies: Vec<String> = login_resp
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok().map(String::from))
        .collect();

    let all_cookies = set_cookies.join("\n");

    assert!(
        all_cookies.contains("seahub-session="),
        "seahub-session cookie should be set:\n{}",
        all_cookies
    );
    assert!(
        all_cookies.contains("sfcsrftoken="),
        "sfcsrftoken cookie should be set:\n{}",
        all_cookies
    );

    // seahub-session is HttpOnly
    let session_header = set_cookies
        .iter()
        .find(|c| c.starts_with("seahub-session="))
        .expect("seahub-session header");
    assert!(
        session_header.contains("HttpOnly"),
        "seahub-session must be HttpOnly: {session_header}"
    );

    // sfcsrftoken is NOT HttpOnly
    let csrf_header = set_cookies
        .iter()
        .find(|c| c.starts_with("sfcsrftoken="))
        .expect("sfcsrftoken header");
    assert!(
        !csrf_header.contains("HttpOnly"),
        "sfcsrftoken must NOT be HttpOnly: {csrf_header}"
    );
}

/// External API clients can still authenticate via `Authorization: Token`.
#[tokio::test]
async fn test_api_auth_token_still_works() {
    let server = TestServer::start().await;
    create_test_user(server.db.as_ref(), "test@example.com", "password").await;

    let api_token = get_api_token(&server).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/api/v2.1/starred-items/", server.base_url))
        .header("Authorization", format!("Token {}", api_token))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "API token auth should still work");
}

/// Browser requests using session cookie + X-CSRFToken header succeed.
#[tokio::test]
async fn test_api_cookie_and_csrf_header() {
    let server = TestServer::start().await;
    create_test_user(server.db.as_ref(), "test@example.com", "password").await;

    let api_token = get_api_token(&server).await;
    let repo_id = create_repo_api(&server.base_url, &api_token, "csrf-test-repo").await;

    // Star a file via API for setup.
    let raw = reqwest::Client::new();
    let star_resp = raw
        .post(format!("{}/api/v2.1/starred-items/", server.base_url))
        .header("Authorization", format!("Token {}", api_token))
        .header("Content-Type", "application/json")
        .body(serde_json::json!({"repo_id": repo_id, "path": "/"}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(star_resp.status(), 200, "star via API");

    // Login via UI.
    let ui_client = ui_login(&server).await;

    // Extract CSRF token from the file browser page (which has hidden form inputs).
    let page_resp = ui_client
        .get(format!(
            "{}/library/{}/csrf-test-repo/",
            server.base_url, repo_id
        ))
        .send()
        .await
        .unwrap();
    let html = page_resp.text().await.unwrap_or_default();
    let csrf_token = html
        .split(r#"name="csrf_token" value=""#)
        .nth(1)
        .and_then(|s| s.split('"').next())
        .unwrap_or("");

    assert!(!csrf_token.is_empty(), "could not extract csrf_token");

    // Call starred list API with session cookie + X-CSRFToken header.
    let list_resp = ui_client
        .get(format!("{}/api/v2.1/starred-items/", server.base_url))
        .header("X-CSRFToken", csrf_token)
        .send()
        .await
        .unwrap();
    assert_eq!(
        list_resp.status(),
        200,
        "cookie + csrf header should work for API calls"
    );
}

/// Wrong X-CSRFToken header is rejected.
#[tokio::test]
async fn test_api_cookie_wrong_csrf_header() {
    let server = TestServer::start().await;
    create_test_user(server.db.as_ref(), "test@example.com", "password").await;

    let ui_client = ui_login(&server).await;

    let resp = ui_client
        .get(format!("{}/api/v2.1/starred-items/", server.base_url))
        .header("X-CSRFToken", "this-is-the-wrong-token-value")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401, "wrong CSRF token should be rejected");
}

/// No X-CSRFToken header with cookie is rejected.
#[tokio::test]
async fn test_api_cookie_no_csrf_header() {
    let server = TestServer::start().await;
    create_test_user(server.db.as_ref(), "test@example.com", "password").await;

    let ui_client = ui_login(&server).await;

    let resp = ui_client
        .get(format!("{}/api/v2.1/starred-items/", server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        401,
        "missing CSRF header should be rejected when using cookie auth"
    );
}

/// No authentication at all is rejected.
#[tokio::test]
async fn test_api_no_auth() {
    let server = TestServer::start().await;
    create_test_user(server.db.as_ref(), "test@example.com", "password").await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{}/api/v2.1/starred-items/", server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401, "no auth should be rejected");
}

/// The `data-token` attribute must NOT appear in the HTML (session token exposure).
#[tokio::test]
async fn test_session_token_not_in_dom() {
    let server = TestServer::start().await;
    create_test_user(server.db.as_ref(), "test@example.com", "password").await;

    let ui_client = ui_login(&server).await;

    // Check the file browser page (the one that previously exposed data-token).
    let api_token = get_api_token(&server).await;
    let repo_id = create_repo_api(&server.base_url, &api_token, "security-test").await;

    let resp = ui_client
        .get(format!(
            "{}/library/{}/security-test/",
            server.base_url, repo_id
        ))
        .send()
        .await
        .unwrap();
    let html = resp.text().await.unwrap_or_default();

    assert!(
        !html.contains("data-token=\""),
        "session_token must NOT appear as data-token in HTML"
    );
}

/// `sfcsrftoken` cookie is set without HttpOnly flag (readable by JavaScript).
#[tokio::test]
async fn test_csrf_token_cookie_not_httponly() {
    let server = TestServer::start().await;
    create_test_user(server.db.as_ref(), "test@example.com", "password").await;

    let client = reqwest::Client::builder()
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let login_resp = client
        .post(format!("{}/accounts/login/", server.base_url))
        .form(&[("email", "test@example.com"), ("password", "password")])
        .send()
        .await
        .unwrap();

    let csrf_cookie = login_resp
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|c| c.starts_with("sfcsrftoken="));

    let csrf_header = csrf_cookie.expect("sfcsrftoken cookie should be set");
    assert!(
        !csrf_header.contains("HttpOnly"),
        "sfcsrftoken must be readable by JS (no HttpOnly), got: {csrf_header}"
    );
}

/// UI form submission with CSRF token still works (backward compatibility).
#[tokio::test]
async fn test_ui_form_csrf_still_works() {
    let server = TestServer::start().await;
    create_test_user(server.db.as_ref(), "test@example.com", "password").await;

    let api_token = get_api_token(&server).await;
    let repo_id = create_repo_api(&server.base_url, &api_token, "form-csrf-test").await;

    // Login via UI.
    let ui_client = ui_login(&server).await;

    // Get a CSRF token from the file browser page (hidden form input).
    let page_resp = ui_client
        .get(format!(
            "{}/library/{}/form-csrf-test/",
            server.base_url, repo_id
        ))
        .send()
        .await
        .unwrap();
    let page_html = page_resp.text().await.unwrap_or_default();
    let csrf_token = page_html
        .split(r#"name="csrf_token" value=""#)
        .nth(1)
        .and_then(|s| s.split('"').next())
        .unwrap_or("");

    assert!(
        !csrf_token.is_empty(),
        "should extract CSRF token from form input"
    );

    // Submit a form to create a directory.
    let form_resp = ui_client
        .post(format!("{}/library/{}/new-dir", server.base_url, repo_id))
        .form(&[("p", "/csrf-test-dir"), ("csrf_token", csrf_token)])
        .send()
        .await
        .unwrap();

    let status = form_resp.status();
    assert!(
        status == 302 || status == 200,
        "form submission should succeed, got {status}"
    );
}
