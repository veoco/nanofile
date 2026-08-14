mod common;

use common::TestFixture;

/// B.7.1 — POST /api2/client-login/
#[tokio::test]
async fn test_client_login_returns_token() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .post_form("/api2/client-login/", Some(&f.api_token), &[])
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(!body["token"].as_str().unwrap_or("").is_empty());
}

#[tokio::test]
async fn test_client_login_unauthorized() {
    let server = common::TestServer::start().await;
    let client = server.client();
    let resp = client.post_form("/api2/client-login/", None, &[]).await;
    assert_eq!(resp.status(), 401);
}

/// B.7.2 — POST /api2/client-sso-link/ (official protocol: anonymous, no body).
///
/// seadroid posts with no auth and no params; the response must be the full
/// browser link (no raw token field).
#[tokio::test]
async fn test_client_sso_link_anonymous() {
    let server = common::TestServer::start().await;
    let client = server.client();

    let resp = client.post_form("/api2/client-sso-link/", None, &[]).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let link = body["link"].as_str().unwrap();
    assert!(
        link.starts_with("http"),
        "link must be an absolute URL: {link}"
    );
    assert!(
        link.contains("/client-sso/"),
        "link must point at /client-sso/: {link}"
    );
    assert!(
        body.get("token").is_none(),
        "response must not leak the token"
    );
}

/// Desktop clients pass `shib_*` device params on the query string.
#[tokio::test]
async fn test_client_sso_link_with_shib_params() {
    let server = common::TestServer::start().await;
    let client = server.client();

    let url = "/api2/client-sso-link/?shib_platform=linux&shib_device_id=dev-1&\
               shib_device_name=pc-1&shib_client_version=9.0.0";
    let resp = client.post_form(url, None, &[]).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["link"].as_str().unwrap().contains("/client-sso/"));
}

/// B.7.3 — GET /api2/client-sso-link/{token}/ (anonymous poll).
#[tokio::test]
async fn test_client_sso_link_poll_waiting() {
    let server = common::TestServer::start().await;
    let client = server.client();

    let token = create_sso_link_token(&client).await;

    let resp = client
        .get(&format!("/api2/client-sso-link/{}/", token), None)
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "waiting");
}

#[tokio::test]
async fn test_client_sso_link_poll_unknown_token() {
    let server = common::TestServer::start().await;
    let client = server.client();

    let resp = client
        .get("/api2/client-sso-link/unknown-token-00000000000000/", None)
        .await;
    assert_eq!(resp.status(), 404);
}

/// Full happy path: create link → browser opens it (marks accessed, bounces to
/// login) → web login → confirm page → POST complete → client polls a success
/// with a camelCase `apiToken`.
#[tokio::test]
async fn test_client_sso_full_flow() {
    let server = common::TestServer::start().await;
    let client = server.client();
    let ui = server.client_ui(); // cookie-tracking browser session

    common::create_test_user(server.db.as_ref(), "test@example.com", "password123").await;

    // 1. Client (anonymous) creates the link.
    let token = create_sso_link_token(&client).await;

    // 2. Browser opens the link → 302 to the login page with next=complete.
    let no_redirect = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let link = format!("{}/client-sso/{}/", server.base_url, token);
    let resp = no_redirect.get(&link).send().await.unwrap();
    assert_eq!(resp.status(), 302);
    let location = resp
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(location.starts_with("/accounts/login/?next="));
    let encoded = location.strip_prefix("/accounts/login/?next=").unwrap();
    let decoded = percent_encoding::percent_decode_str(encoded)
        .decode_utf8()
        .unwrap();
    assert_eq!(
        decoded.as_ref(),
        format!("/client-sso/{}/complete/", token),
        "login next should point at the complete page: {location}"
    );

    // 3. Log in through the web UI (session cookie).
    let resp = ui
        .post_ui_form(
            "/accounts/login/",
            &[("email", "test@example.com"), ("password", "password123")],
        )
        .await;
    assert!(resp.status().is_success(), "web login should succeed");

    // 4. Visit the confirm page → confirm form with a CSRF token.
    let complete_url = format!("/client-sso/{}/complete/", token);
    let resp = ui.get_ui(&complete_url).await;
    assert_eq!(
        resp.status(),
        200,
        "confirm page requires an active session"
    );
    let html = resp.text().await.unwrap();
    assert!(html.contains("Do you want to login to your client?"));
    let csrf = extract_form_csrf(&html);
    assert!(!csrf.is_empty(), "confirm page should embed a CSRF token");

    // 5. Confirm → completion page.
    let resp = ui
        .post_ui_form(&complete_url, &[("csrf_token", csrf.as_str())])
        .await;
    assert_eq!(resp.status(), 200, "completion page should render");

    // 6. Client polls → success with username + camelCase apiToken.
    let resp = client
        .get(&format!("/api2/client-sso-link/{}/", token), None)
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "success");
    assert_eq!(body["username"], "test@example.com");
    let api_token = body["apiToken"].as_str().unwrap().to_string();
    assert_eq!(api_token.len(), 40);

    // The minted apiToken works for authenticated API calls.
    let resp = client.ping(&api_token).await;
    assert_eq!(resp.status(), 200);
}

/// The browser link can only be opened once.
#[tokio::test]
async fn test_client_sso_link_already_visited() {
    let server = common::TestServer::start().await;
    let client = server.client();

    let token = create_sso_link_token(&client).await;
    let link = format!("{}/client-sso/{}/", server.base_url, token);

    let no_redirect = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let resp = no_redirect.get(&link).send().await.unwrap();
    assert_eq!(resp.status(), 302, "first visit should bounce to login");

    let resp = no_redirect.get(&link).send().await.unwrap();
    assert_eq!(resp.status(), 400, "second visit should error");
    let html = resp.text().await.unwrap();
    assert!(html.contains("already been visited"));
}

/// A completed token whose `accessed_at` fell out of the 300s window reports
/// `{"status":"error"}` on poll (matches seahub's timeout handling).
#[tokio::test]
async fn test_client_sso_poll_expired() {
    let server = common::TestServer::start().await;
    let client = server.client();

    let token = create_sso_link_token(&client).await;
    let now = chrono::Utc::now().timestamp();

    // Simulate: the link was accessed but the completion window elapsed before
    // the browser confirmed. The poll must report "error", not success.
    server
        .repos
        .sso_login_token
        .mark_accessed(&token, now - 400)
        .await
        .unwrap();
    server
        .repos
        .sso_login_token
        .complete(
            &token,
            "test@example.com",
            "0123456789012345678901234567890123456789",
        )
        .await
        .unwrap();

    let resp = client
        .get(&format!("/api2/client-sso-link/{}/", token), None)
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "error");
}

/// The complete page requires a web session.
#[tokio::test]
async fn test_client_sso_complete_requires_login() {
    let server = common::TestServer::start().await;
    let client = server.client();

    let token = create_sso_link_token(&client).await;

    // Use a no-redirect client so the WebUser redirect surfaces directly.
    let no_redirect = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let resp = no_redirect
        .get(format!(
            "{}/client-sso/{}/complete/",
            server.base_url, token
        ))
        .send()
        .await
        .unwrap();
    // WebUser rejection redirects to the login page (axum Redirect = 303).
    assert_eq!(resp.status(), 303);
    let location = resp.headers().get("location").unwrap().to_str().unwrap();
    assert_eq!(location, "/accounts/login/");
}

/// The complete POST requires a valid CSRF token.
#[tokio::test]
async fn test_client_sso_complete_csrf_required() {
    let server = common::TestServer::start().await;
    let ui = server.client_ui();

    common::create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let client = server.client();
    let token = create_sso_link_token(&client).await;

    // Log in and open the browser link so the token is accessed.
    let resp = ui
        .post_ui_form(
            "/accounts/login/",
            &[("email", "test@example.com"), ("password", "password123")],
        )
        .await;
    assert!(resp.status().is_success());
    let no_redirect = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let link = format!("{}/client-sso/{}/", server.base_url, token);
    let _ = no_redirect.get(&link).send().await.unwrap();

    // POST without a CSRF token → rejected.
    let complete_url = format!("/client-sso/{}/complete/", token);
    let resp = ui.post_ui_form(&complete_url, &[]).await;
    assert_eq!(resp.status(), 400);
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Create a link via the anonymous POST and return the token extracted from
/// the returned link (same last-path-segment extraction seadroid uses).
async fn create_sso_link_token(client: &common::client::TestClient) -> String {
    let resp = client.post_form("/api2/client-sso-link/", None, &[]).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let link = body["link"].as_str().unwrap().to_string();
    link.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap()
        .to_string()
}

/// Extract the `csrf_token` hidden input value from a rendered form page.
fn extract_form_csrf(html: &str) -> String {
    let marker = r#"name="csrf_token" value=""#;
    html.find(marker)
        .and_then(|i| {
            let rest = &html[i + marker.len()..];
            let end = rest.find('"')?;
            Some(rest[..end].to_string())
        })
        .unwrap_or_default()
}

/// B.8.1 — POST /api2/device-wiped/ (official protocol: anonymous + the
/// device's own API token).
#[tokio::test]
async fn test_device_wiped() {
    let f = TestFixture::new().await;

    // Login with device metadata so the API token is tied to a device.
    let resp = f
        .client
        .post_form(
            "/api2/auth-token/",
            None,
            &[
                ("username", f.email.as_str()),
                ("password", f.password.as_str()),
                ("platform", "linux"),
                ("device_id", "test-device"),
                ("device_name", "laptop"),
                ("client_version", "9.0.0"),
            ],
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let device_token = body["token"].as_str().unwrap().to_string();

    let resp = f
        .client
        .post_form("/api2/device-wiped/", None, &[("token", &device_token)])
        .await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_device_wiped_invalid_token() {
    let server = common::TestServer::start().await;
    let client = server.client();
    // A nonexistent / anonymous report must be rejected.
    let resp = client
        .post_form(
            "/api2/device-wiped/",
            None,
            &[("token", "nonexistent-token-00000000000000000000")],
        )
        .await;
    assert_eq!(resp.status(), 400);

    // Missing token entirely.
    let resp = client.post_form("/api2/device-wiped/", None, &[]).await;
    assert_eq!(resp.status(), 400);
}

/// B.9.1 — GET /api2/search/?q=&per_page=&page=&search_repo=
///
/// Searches file/directory names across accessible repos using case-insensitive
/// substring matching.
#[tokio::test]
async fn test_search_returns_results() {
    let f = TestFixture::new().await;

    // Upload files to the repo
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "hello.txt", b"hello world")
        .await;
    assert_eq!(resp.status(), 200, "file upload should succeed");

    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "readme.md", b"# Readme")
        .await;
    assert_eq!(resp.status(), 200, "file upload should succeed");

    // Search for "hello" — should find hello.txt
    let resp = f
        .client
        .get("/api2/search/?q=hello&per_page=10", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let results = body["results"].as_array().unwrap();
    assert_eq!(results.len(), 1, "should find 1 file matching 'hello'");
    assert_eq!(results[0]["name"], "hello.txt");
    assert_eq!(results[0]["repo_id"], f.repo_id);
    assert_eq!(results[0]["fullpath"], "/hello.txt");
    assert_eq!(results[0]["is_dir"], false);
    assert!(results[0]["last_modified"].as_i64().unwrap() > 0);
    assert!(results[0]["size"].as_i64().unwrap() > 0);
    assert!(!results[0]["oid"].as_str().unwrap().is_empty());
    assert!(!results[0]["repo_name"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn test_search_case_insensitive() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "HELLO.TXT", b"hello")
        .await;
    assert_eq!(resp.status(), 200);

    // Search lowercase should match uppercase filename
    let resp = f
        .client
        .get("/api2/search/?q=hello", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let results = body["results"].as_array().unwrap();
    assert_eq!(
        results.len(),
        1,
        "case-insensitive match should find HELLO.TXT"
    );
    assert_eq!(results[0]["name"], "HELLO.TXT");
}

#[tokio::test]
async fn test_search_all_repos() {
    let f = TestFixture::new().await;

    // Upload a file
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "hello.txt", b"hello")
        .await;
    assert_eq!(resp.status(), 200);

    // Create a second repo and upload a file with same name
    let repo2_id = common::create_test_repo(&f.client, &f.api_token, "second-repo").await;
    let resp = f
        .client
        .upload_file(&f.api_token, &repo2_id, "/", "hello.txt", b"hello")
        .await;
    assert_eq!(resp.status(), 200);

    // Search across all repos — should find in both
    let resp = f
        .client
        .get("/api2/search/?q=hello&per_page=10", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let results = body["results"].as_array().unwrap();
    assert_eq!(results.len(), 2, "should find 'hello' in both repos");
    let repo_ids: Vec<&str> = results
        .iter()
        .map(|r| r["repo_id"].as_str().unwrap())
        .collect();
    assert!(repo_ids.contains(&&f.repo_id[..]));
    assert!(repo_ids.contains(&&repo2_id[..]));
}

#[tokio::test]
async fn test_search_scoped_to_repo() {
    let f = TestFixture::new().await;

    // Upload to first repo
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "hello.txt", b"hello")
        .await;
    assert_eq!(resp.status(), 200);

    // Create second repo and upload there too
    let repo2_id = common::create_test_repo(&f.client, &f.api_token, "repo2").await;
    let resp = f
        .client
        .upload_file(&f.api_token, &repo2_id, "/", "world.txt", b"world")
        .await;
    assert_eq!(resp.status(), 200);

    // Search scoped to first repo — should find hello but not world
    let url = format!("/api2/search/?q=hello&search_repo={}", f.repo_id);
    let resp = f.client.get(&url, Some(&f.api_token)).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let results = body["results"].as_array().unwrap();
    assert_eq!(results.len(), 1, "scoped search should find 1 result");
    assert_eq!(results[0]["name"], "hello.txt");
    assert_eq!(results[0]["repo_id"], f.repo_id);

    // scoped to repo2 — empty for "hello"
    let url = format!("/api2/search/?q=hello&search_repo={}", repo2_id);
    let resp = f.client.get(&url, Some(&f.api_token)).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let results = body["results"].as_array().unwrap();
    assert!(
        results.is_empty(),
        "scoped search should find nothing in other repo"
    );
}

#[tokio::test]
async fn test_search_pagination() {
    let f = TestFixture::new().await;

    // Upload 5 files with searchable names
    for i in 0..5 {
        let name = format!("alpha-{}.txt", i);
        let resp = f
            .client
            .upload_file(&f.api_token, &f.repo_id, "/", &name, b"data")
            .await;
        assert_eq!(resp.status(), 200);
    }

    // Page 1: per_page=2
    let resp = f
        .client
        .get(
            "/api2/search/?q=alpha&per_page=2&page=1",
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["results"].as_array().unwrap().len(),
        2,
        "page 1 should have 2 results"
    );
    assert_eq!(body["total"], 5, "total should be 5");
    assert_eq!(body["has_more"], true, "page 1 should have more");

    // Page 2: per_page=2
    let resp = f
        .client
        .get(
            "/api2/search/?q=alpha&per_page=2&page=2",
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["results"].as_array().unwrap().len(),
        2,
        "page 2 should have 2 results"
    );
    assert_eq!(body["has_more"], true, "page 2 should have more");

    // Page 3: per_page=2 — should return 1 result (5th file)
    let resp = f
        .client
        .get(
            "/api2/search/?q=alpha&per_page=2&page=3",
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["results"].as_array().unwrap().len(),
        1,
        "page 3 should have 1 result"
    );
    assert_eq!(body["has_more"], false, "page 3 should not have more");

    // Page 4: beyond end — empty
    let resp = f
        .client
        .get(
            "/api2/search/?q=alpha&per_page=2&page=4",
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body["results"].as_array().unwrap().is_empty(),
        "beyond last page should be empty"
    );
    assert_eq!(body["has_more"], false);
}

#[tokio::test]
async fn test_search_directories() {
    let f = TestFixture::new().await;

    // Create a directory
    let resp = f
        .client
        .create_dir(&f.api_token, &f.repo_id, "/mydir")
        .await;
    assert_eq!(resp.status(), 200, "create dir should succeed");

    // Upload a file into it
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/mydir", "inner.txt", b"data")
        .await;
    assert_eq!(resp.status(), 200, "file upload into subdir should succeed");

    // Search for "mydir" — should find the directory
    let resp = f
        .client
        .get("/api2/search/?q=mydir", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let results = body["results"].as_array().unwrap();
    let dirs: Vec<&serde_json::Value> = results.iter().filter(|r| r["is_dir"] == true).collect();
    assert_eq!(dirs.len(), 1, "should find 1 directory matching 'mydir'");
    assert_eq!(dirs[0]["name"], "mydir");
    assert_eq!(dirs[0]["fullpath"], "/mydir");

    // Search for "inner" — should find the file inside
    let resp = f
        .client
        .get("/api2/search/?q=inner", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    // The inner.txt file is inside /mydir/inner.txt
    let all_results = body["results"].as_array().unwrap();
    assert!(
        all_results.iter().any(|r| r["name"] == "inner.txt"),
        "should find inner.txt"
    );
}

#[tokio::test]
async fn test_search_no_keyword() {
    let f = TestFixture::new().await;

    // Empty query should return empty results
    let resp = f
        .client
        .get("/api2/search/?q=&per_page=10", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["results"].as_array().unwrap().is_empty());
    assert_eq!(body["total"], 0);

    // Missing q should also return empty
    let resp = f
        .client
        .get("/api2/search/?per_page=10", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["results"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_search_no_matches() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "hello.txt", b"hello")
        .await;
    assert_eq!(resp.status(), 200);

    // Non-existent keyword
    let resp = f
        .client
        .get("/api2/search/?q=zzzzz_not_found", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["results"].as_array().unwrap().is_empty());
    assert_eq!(body["total"], 0);
    assert_eq!(body["has_more"], false);
}

#[tokio::test]
async fn test_search_unauthorized() {
    let server = common::TestServer::start().await;
    let client = server.client();
    let resp = client.get("/api2/search/?q=test", None).await;
    assert_eq!(resp.status(), 401);
}
