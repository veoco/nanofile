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

/// B.7.2 — POST /api2/client-sso-link/
#[tokio::test]
async fn test_client_sso_link() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .post_form(
            "/api2/client-sso-link/",
            Some(&f.api_token),
            &[
                ("platform", "linux"),
                ("device_id", "test-device"),
                ("device_name", "test-pc"),
            ],
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(!body["link"].as_str().unwrap_or("").is_empty());
    assert!(!body["token"].as_str().unwrap_or("").is_empty());
}

/// B.7.3 — GET /api2/client-sso-link/{token}/
#[tokio::test]
async fn test_client_sso_link_poll() {
    let f = TestFixture::new().await;

    // Create a link first
    let resp = f
        .client
        .post_form(
            "/api2/client-sso-link/",
            Some(&f.api_token),
            &[
                ("platform", "linux"),
                ("device_id", "device-1"),
                ("device_name", "pc-1"),
            ],
        )
        .await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    // Poll the link — should be pending
    let resp = f
        .client
        .get(
            &format!("/api2/client-sso-link/{}/", token),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "pending");
}

/// B.8.1 — POST /api2/device-wiped/
#[tokio::test]
async fn test_device_wiped() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .post_json(
            "/api2/device-wiped/",
            Some(&f.api_token),
            &serde_json::json!({
                "device_id": "test-device",
                "platform": "linux",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_device_wiped_unauthorized() {
    let server = common::TestServer::start().await;
    let client = server.client();
    let resp = client
        .post_json(
            "/api2/device-wiped/",
            None,
            &serde_json::json!({
                "device_id": "test",
                "platform": "linux",
            }),
        )
        .await;
    assert_eq!(resp.status(), 401);
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
