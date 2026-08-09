mod common;

use common::TestFixture;
use common::create_test_user;

/// Insert a `repo_members` row granting `user_id` the given permission.
async fn add_member(f: &TestFixture, user_id: i32, permission: &str) {
    use sea_orm::ActiveModelTrait;
    infra::entity::repo_member::ActiveModel {
        id: sea_orm::NotSet,
        repo_id: sea_orm::Set(f.repo_id.clone()),
        user_id: sea_orm::Set(user_id),
        permission: sea_orm::Set(permission.to_string()),
        created_at: sea_orm::Set(chrono::Utc::now().timestamp()),
    }
    .insert(f.server.db.as_ref())
    .await
    .unwrap();
}

/// U.1 — POST /api/v2.1/upload-links/ → create basic upload link
#[tokio::test]
async fn test_upload_link_create_basic() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .post_json(
            "/api/v2.1/upload-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200, "create upload link failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        !body["token"].as_str().unwrap_or("").is_empty(),
        "token must not be empty"
    );
}

/// U.2 — POST /api/v2.1/upload-links/ with password
#[tokio::test]
async fn test_upload_link_create_with_password() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .post_json(
            "/api/v2.1/upload-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/",
                "password": "uploadpass",
            }),
        )
        .await;
    assert_eq!(
        resp.status(),
        200,
        "create upload link with password failed"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();
    assert!(!token.is_empty());

    // List and verify has_password
    let list = f
        .client
        .get("/api/v2.1/upload-links/", Some(&f.api_token))
        .await;
    assert_eq!(list.status(), 200);
    let list_body: serde_json::Value = list.json().await.unwrap();
    let links = list_body.as_array().unwrap();
    let ul = links.iter().find(|l| l["token"] == token).unwrap();
    assert_eq!(
        ul["has_password"], true,
        "upload link should have has_password=true"
    );
}

/// U.3 — POST /api/v2.1/upload-links/ with description
#[tokio::test]
async fn test_upload_link_create_with_description() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .post_json(
            "/api/v2.1/upload-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/",
                "description": "test description",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    // List and verify description
    let list = f
        .client
        .get("/api/v2.1/upload-links/", Some(&f.api_token))
        .await;
    assert_eq!(list.status(), 200);
    let list_body: serde_json::Value = list.json().await.unwrap();
    let links = list_body.as_array().unwrap();
    let ul = links.iter().find(|l| l["token"] == token).unwrap();
    assert_eq!(ul["description"], "test description");
}

/// U.4 — GET /api/v2.1/upload-links/ with repo_id and path filtering
#[tokio::test]
async fn test_upload_link_list_filter_by_repo_and_path() {
    let f = TestFixture::new().await;

    // Create an upload link for root path
    let _resp = f
        .client
        .post_json(
            "/api/v2.1/upload-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/",
            }),
        )
        .await;

    // List with repo_id and path = "/" — should find the link
    let list = f
        .client
        .get(
            &format!("/api/v2.1/upload-links/?repo_id={}&path=/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(list.status(), 200);
    let body: serde_json::Value = list.json().await.unwrap();
    let links = body.as_array().unwrap();
    assert!(!links.is_empty(), "should find upload link for this path");

    // List with non-matching path — should return empty
    let list2 = f
        .client
        .get(
            &format!(
                "/api/v2.1/upload-links/?repo_id={}&path=/nonexistent",
                f.repo_id
            ),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(list2.status(), 200);
    let body2: serde_json::Value = list2.json().await.unwrap();
    let links2 = body2.as_array().unwrap();
    assert!(
        links2.is_empty(),
        "should not find upload link for non-matching path"
    );
}

/// U.5 — GET /api/v2.1/upload-links/ list response fields
#[tokio::test]
async fn test_upload_link_list_response_fields() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .post_json(
            "/api/v2.1/upload-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);

    let list = f
        .client
        .get("/api/v2.1/upload-links/", Some(&f.api_token))
        .await;
    assert_eq!(list.status(), 200);
    let body: serde_json::Value = list.json().await.unwrap();
    let links = body.as_array().unwrap();
    assert!(!links.is_empty(), "should have at least one upload link");

    let link = &links[0];
    assert!(
        !link["token"].as_str().unwrap_or("").is_empty(),
        "token missing"
    );
    assert!(
        link["has_password"].is_boolean(),
        "has_password should be boolean"
    );
    assert!(
        link.get("expire_at").is_some(),
        "expire_at should be present"
    );
    assert!(link.get("view_cnt").is_some(), "view_cnt should be present");
}

/// U.6 — GET /api/v2.1/upload-links/{token}/ — get upload link detail
#[tokio::test]
async fn test_upload_link_get_detail() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .post_json(
            "/api/v2.1/upload-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let token = resp.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    // Get detail
    let detail = f
        .client
        .get(
            &format!("/api/v2.1/upload-links/{}/", token),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(detail.status(), 200);
    let body: serde_json::Value = detail.json().await.unwrap();
    assert_eq!(body["token"], token);
    assert_eq!(body["repo_id"], f.repo_id);
    assert_eq!(body["path"], "/");
    assert!(body["view_cnt"].is_number(), "view_cnt should be a number");
    assert_eq!(body["has_password"], false);
}

/// U.7 — PUT /api/v2.1/upload-links/{token}/ — update password
#[tokio::test]
async fn test_upload_link_update_password() {
    let f = TestFixture::new().await;

    // Create with no password
    let resp = f
        .client
        .post_json(
            "/api/v2.1/upload-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    // Set password
    let upd = f
        .client
        .put_json(
            &format!("/api/v2.1/upload-links/{}/", token),
            Some(&f.api_token),
            &serde_json::json!({"password": "newpass"}),
        )
        .await;
    assert_eq!(upd.status(), 200, "update password should succeed");

    // Verify has_password changed
    let detail = f
        .client
        .get(
            &format!("/api/v2.1/upload-links/{}/", token),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(detail.status(), 200);
    let body: serde_json::Value = detail.json().await.unwrap();
    assert_eq!(body["has_password"], true, "password should be set");
}

/// U.8 — PUT /api/v2.1/upload-links/{token}/ — clear password
#[tokio::test]
async fn test_upload_link_clear_password() {
    let f = TestFixture::new().await;

    // Create with password
    let resp = f
        .client
        .post_json(
            "/api/v2.1/upload-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/",
                "password": "secret",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    // Clear password (null → Some(None))
    let upd = f
        .client
        .put_json(
            &format!("/api/v2.1/upload-links/{}/", token),
            Some(&f.api_token),
            &serde_json::json!({"password": null}),
        )
        .await;
    assert_eq!(upd.status(), 200, "clear password should succeed");

    // Verify has_password changed
    let detail = f
        .client
        .get(
            &format!("/api/v2.1/upload-links/{}/", token),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(detail.status(), 200);
    let body: serde_json::Value = detail.json().await.unwrap();
    assert_eq!(body["has_password"], false, "password should be cleared");
}

/// U.9 — PUT /api/v2.1/upload-links/{token}/ — update description
#[tokio::test]
async fn test_upload_link_update_description() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .post_json(
            "/api/v2.1/upload-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    // Update description
    let upd = f
        .client
        .put_json(
            &format!("/api/v2.1/upload-links/{}/", token),
            Some(&f.api_token),
            &serde_json::json!({"description": "updated desc"}),
        )
        .await;
    assert_eq!(upd.status(), 200, "update description should succeed");

    // Verify description changed
    let detail = f
        .client
        .get(
            &format!("/api/v2.1/upload-links/{}/", token),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(detail.status(), 200);
    let body: serde_json::Value = detail.json().await.unwrap();
    assert_eq!(body["description"], "updated desc");
}

/// U.10 — PUT /api/v2.1/upload-links/{token}/ — update by non-owner returns 404
#[tokio::test]
async fn test_upload_link_update_other_fails() {
    let f = TestFixture::new().await;
    let db = &*f.server.db;

    // Create upload link as user1
    let resp = f
        .client
        .post_json(
            "/api/v2.1/upload-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    // Create a second user
    let _user2_id = create_test_user(db, "user2-ul@test.com", "password2").await;
    let resp2 = f.client.login("user2-ul@test.com", "password2").await;
    assert_eq!(resp2.status(), 200);
    let token2_val: serde_json::Value = resp2.json().await.unwrap();
    let api_token2 = token2_val["token"].as_str().unwrap().to_string();

    // User2 tries to update user1's upload link
    let upd = f
        .client
        .put_json(
            &format!("/api/v2.1/upload-links/{}/", token),
            Some(&api_token2),
            &serde_json::json!({"description": "hacked"}),
        )
        .await;
    assert_eq!(upd.status(), 404, "non-owner update must return 404");
}

/// U.11 — PUT /api/v2.1/upload-links/{token}/ — update non-existent token returns 404
#[tokio::test]
async fn test_upload_link_update_nonexistent() {
    let f = TestFixture::new().await;

    let upd = f
        .client
        .put_json(
            "/api/v2.1/upload-links/nonexistent-token/",
            Some(&f.api_token),
            &serde_json::json!({"description": "test"}),
        )
        .await;
    assert_eq!(upd.status(), 404, "non-existent token must return 404");
}

/// U.12 — DELETE /api/v2.1/upload-links/{token}/ — delete by creator
#[tokio::test]
async fn test_upload_link_delete_own() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .post_json(
            "/api/v2.1/upload-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    // List should have 1 link
    let list = f
        .client
        .get("/api/v2.1/upload-links/", Some(&f.api_token))
        .await;
    assert_eq!(list.status(), 200);
    let list_body: serde_json::Value = list.json().await.unwrap();
    assert_eq!(list_body.as_array().unwrap().len(), 1);

    // Delete
    let del = f
        .client
        .delete(
            &format!("/api/v2.1/upload-links/{}/", token),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(del.status(), 200, "delete should succeed");

    // Verify it's gone
    let detail = f
        .client
        .get(
            &format!("/api/v2.1/upload-links/{}/", token),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(detail.status(), 404, "deleted link should be gone");
}

/// U.13 — DELETE /api/v2.1/upload-links/{token}/ — delete by other user returns 404
#[tokio::test]
async fn test_upload_link_delete_other_fails() {
    let f = TestFixture::new().await;
    let db = &*f.server.db;

    // Create upload link as user1
    let resp = f
        .client
        .post_json(
            "/api/v2.1/upload-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    // Create second user
    let _user2_id = create_test_user(db, "user2-del@test.com", "password2").await;
    let resp2 = f.client.login("user2-del@test.com", "password2").await;
    assert_eq!(resp2.status(), 200);
    let token2_val: serde_json::Value = resp2.json().await.unwrap();
    let api_token2 = token2_val["token"].as_str().unwrap().to_string();

    // User2 tries to delete user1's link
    let del = f
        .client
        .delete(
            &format!("/api/v2.1/upload-links/{}/", token),
            Some(&api_token2),
        )
        .await;
    assert_eq!(del.status(), 403, "other user's delete must return 403");

    // Verify link still exists
    let detail = f
        .client
        .get(
            &format!("/api/v2.1/upload-links/{}/", token),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(detail.status(), 200, "link should still be valid");
}

/// U.14 — GET /api/v2.1/upload-links/{token}/upload/ — get upload URL
#[tokio::test]
async fn test_upload_link_get_upload_url() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .post_json(
            "/api/v2.1/upload-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    // Get upload URL
    let url_resp = f
        .client
        .get(
            &format!("/api/v2.1/upload-links/{}/upload/", token),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(url_resp.status(), 200, "should return upload URL");
    let url_body: serde_json::Value = url_resp.json().await.unwrap();
    let upload_link = url_body["upload_link"].as_str().unwrap();
    assert!(
        upload_link.starts_with("/upload-aj/"),
        "upload link should start with /upload-aj/"
    );
    assert!(
        upload_link.len() > "/upload-aj/".len(),
        "upload link should contain a token"
    );
}

/// Security: a non-member must not be able to create an upload link into
/// someone else's repo.
#[tokio::test]
async fn test_upload_link_create_non_member_forbidden() {
    let f = TestFixture::new().await;
    create_test_user(f.server.db.as_ref(), "other@example.com", "password123").await;
    let resp = f.client.login("other@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let b_token = body["token"].as_str().unwrap().to_string();

    let resp = f
        .client
        .post_json(
            "/api/v2.1/upload-links/",
            Some(&b_token),
            &serde_json::json!({ "repo_id": f.repo_id, "path": "/" }),
        )
        .await;
    assert_eq!(resp.status(), 403);
}

/// Security: a read-only member must not be able to create an anonymous
/// *write* upload link into the repo (previously only read permission was
/// checked, letting read-only members mint links that bypass their role).
#[tokio::test]
async fn test_upload_link_create_readonly_member_forbidden() {
    let f = TestFixture::new().await;
    let b_id = create_test_user(f.server.db.as_ref(), "ro@example.com", "password123").await;
    add_member(&f, b_id, "r").await;

    let resp = f.client.login("ro@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let b_token = body["token"].as_str().unwrap().to_string();

    let resp = f
        .client
        .post_json(
            "/api/v2.1/upload-links/",
            Some(&b_token),
            &serde_json::json!({ "repo_id": f.repo_id, "path": "/" }),
        )
        .await;
    assert_eq!(resp.status(), 403);
}

/// Security: the no-token web upload endpoints trust a client-supplied
/// repo_id, so a non-member with a valid session must be rejected (H1 —
/// previously any authenticated user could upload into any repo).
#[tokio::test]
async fn test_web_upload_rejects_non_member() {
    let server = common::TestServer::start().await;

    // Owner: user + repo.
    let _owner_id = create_test_user(&server.db, "owner@example.com", "ownerpass").await;
    let owner_client = server.client();
    let resp = owner_client.login("owner@example.com", "ownerpass").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let owner_token = body["token"].as_str().unwrap().to_string();
    let repo_id = common::create_test_repo(&owner_client, &owner_token, "victim-repo").await;

    // Intruder: a valid user who is NOT a member of the repo.
    create_test_user(&server.db, "intruder@example.com", "intruderpass").await;

    // Browser session for the intruder (WebUser). Disable redirect following
    // so the login response's 302 isn't consumed into the landing page.
    let client = reqwest::Client::builder()
        .cookie_store(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let resp = client
        .post(format!("{}/accounts/login/", server.base_url))
        .form(&[
            ("email", "intruder@example.com"),
            ("password", "intruderpass"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 302, "login should succeed");

    // Try to upload into the owner's repo via the no-token /upload-aj/ endpoint.
    let part = reqwest::multipart::Part::bytes(b"hello".to_vec()).file_name("pwned.txt");
    let form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("repo_id", repo_id.clone())
        .text("parent_dir", "/")
        .text("relative_path", "");
    let resp = client
        .post(format!("{}/upload-aj/", server.base_url))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403, "non-member upload must be rejected");
}

/// Security: a password-protected upload link must not yield an upload URL
/// without the password (previously the password was never checked).
#[tokio::test]
async fn test_upload_link_upload_url_requires_password() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .post_json(
            "/api/v2.1/upload-links/",
            Some(&f.api_token),
            &serde_json::json!({ "repo_id": f.repo_id, "path": "/", "password": "uploadpass" }),
        )
        .await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    // No password / session flag → must be rejected.
    let url_resp = f
        .client
        .get(&format!("/api/v2.1/upload-links/{}/upload/", token), None)
        .await;
    assert_eq!(url_resp.status(), 403);
}

/// The official flow: submitting the password via the web form sets a session
/// flag; the upload URL is then granted to that session.
#[tokio::test]
async fn test_upload_link_upload_url_after_password_form() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .post_json(
            "/api/v2.1/upload-links/",
            Some(&f.api_token),
            &serde_json::json!({ "repo_id": f.repo_id, "path": "/", "password": "uploadpass" }),
        )
        .await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    // Browser session with cookie store.
    let browser = common::client::TestClient::new_with_cookies(&f.server.base_url);

    // Submit the password via the web form. The cookie-enabled client follows
    // the redirect, so we accept either the redirect or the final page.
    let resp = browser
        .post_ui_form(&format!("/u/{}/", token), &[("password", "uploadpass")])
        .await;
    assert!(
        resp.status().is_redirection() || resp.status().is_success(),
        "password POST should redirect or succeed, got {}",
        resp.status()
    );

    // The same session can now obtain an upload URL.
    let url_resp = browser
        .get(&format!("/api/v2.1/upload-links/{}/upload/", token), None)
        .await;
    assert_eq!(
        url_resp.status(),
        200,
        "upload URL should be granted after password"
    );

    // But a fresh session still cannot.
    let fresh = common::client::TestClient::new_with_cookies(&f.server.base_url);
    let url_resp = fresh
        .get(&format!("/api/v2.1/upload-links/{}/upload/", token), None)
        .await;
    assert_eq!(url_resp.status(), 403);
}

/// U.15 — GET /api/v2.1/upload-links/{token}/ — non-existent token returns 404
#[tokio::test]
async fn test_upload_link_get_nonexistent() {
    let f = TestFixture::new().await;

    let detail = f
        .client
        .get(
            "/api/v2.1/upload-links/nonexistent-token/",
            Some(&f.api_token),
        )
        .await;
    assert_eq!(
        detail.status(),
        404,
        "non-existent upload link must return 404"
    );
}

/// U.16 — Upload link list by repo
#[tokio::test]
async fn test_upload_link_list_repo_links() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .post_json(
            "/api/v2.1/upload-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);

    // List repo upload links
    let list = f
        .client
        .get(
            &format!("/api/v2.1/repos/{}/upload-links/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(list.status(), 200);
    let body: serde_json::Value = list.json().await.unwrap();
    let links = body.as_array().unwrap();
    assert!(!links.is_empty(), "should find upload link for this repo");
    assert_eq!(links[0]["repo_id"], f.repo_id);
}

/// U.17 — Upload link with expired token should deny access
#[tokio::test]
async fn test_upload_link_view_page_expired() {
    let f = TestFixture::new().await;

    // Create upload link with negative expire_days (past expiry)
    let resp = f
        .client
        .post_json(
            "/api/v2.1/upload-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/",
                "expire_days": -1,
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    // Access upload page — should 404 because link is expired
    let page = f.client.get(&format!("/u/{}/", token), None).await;
    assert_eq!(page.status(), 404, "expired upload link must return 404");
}

/// U.18 — DELETE /api/v2.1/upload-links/clean-invalid/ — removes expired
/// links while keeping valid ones.
#[tokio::test]
async fn test_upload_link_clean_invalid_removes_expired() {
    let f = TestFixture::new().await;

    // Valid link (repo exists, not expired) → must survive.
    let resp = f
        .client
        .post_json(
            "/api/v2.1/upload-links/",
            Some(&f.api_token),
            &serde_json::json!({"repo_id": f.repo_id, "path": "/"}),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let keep_token = body["token"].as_str().unwrap().to_string();

    // Expired link (negative expire_days) → must be cleaned.
    let resp2 = f
        .client
        .post_json(
            "/api/v2.1/upload-links/",
            Some(&f.api_token),
            &serde_json::json!({"repo_id": f.repo_id, "path": "/", "expire_days": -1}),
        )
        .await;
    assert_eq!(resp2.status(), 200);
    let body2: serde_json::Value = resp2.json().await.unwrap();
    let expired_token = body2["token"].as_str().unwrap().to_string();

    let clean = f
        .client
        .delete("/api/v2.1/upload-links/clean-invalid/", Some(&f.api_token))
        .await;
    assert_eq!(clean.status(), 200);
    let clean_body: serde_json::Value = clean.json().await.unwrap();
    assert_eq!(clean_body["deleted"], 1);

    // Valid link survives, expired link is gone.
    let keep = f
        .client
        .get(
            &format!("/api/v2.1/upload-links/{}/", keep_token),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(keep.status(), 200);
    let gone = f
        .client
        .get(
            &format!("/api/v2.1/upload-links/{}/", expired_token),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(gone.status(), 404);
}
