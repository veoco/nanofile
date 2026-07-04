mod common;

use common::TestFixture;
use common::create_test_user;

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
    let links = list_body["upload_link_list"].as_array().unwrap();
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
    let links = list_body["upload_link_list"].as_array().unwrap();
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
    let links = body["upload_link_list"].as_array().unwrap();
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
    let links2 = body2["upload_link_list"].as_array().unwrap();
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
    let links = body["upload_link_list"].as_array().unwrap();
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
    assert_eq!(list_body["upload_link_list"].as_array().unwrap().len(), 1);

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
    let links = body["upload_link_list"].as_array().unwrap();
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
