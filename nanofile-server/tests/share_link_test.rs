mod common;

use common::TestFixture;
use common::create_test_user;

/// H.1 — POST /api/v2.1/share-links/ → GET /f/{token}/ download
#[tokio::test]
async fn test_share_link_create_and_download() {
    let f = TestFixture::new().await;

    // Upload a file to share
    let up = f
        .client
        .upload_file(
            &f.api_token,
            &f.repo_id,
            "/",
            "shared.txt",
            b"share content",
        )
        .await;
    assert!(up.status().is_success(), "upload failed");

    // Create share link via v2.1 API
    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/shared.txt",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200, "create share link failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();
    assert!(!token.is_empty(), "token must not be empty");

    // Download via shared link (no auth, no password)
    let dl = f.client.get(&format!("/f/{}/", token), None).await;
    assert_eq!(dl.status(), 200, "shared download should succeed");
    let content = dl.bytes().await.unwrap();
    assert_eq!(&content[..], b"share content");
}

/// H.2 — Share link with password protection
#[tokio::test]
async fn test_share_link_with_password() {
    let f = TestFixture::new().await;

    // Upload a file (plain filename, no leading slash)
    let up = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "secret.txt", b"secret data")
        .await;
    assert!(up.status().is_success(), "upload failed");

    // Create password-protected share link via v2.1 API
    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/secret.txt",
                "password": "mypassword",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200, "create share link failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    // Download WITHOUT password → should return 400
    let dl = f.client.get(&format!("/f/{}/", token), None).await;
    assert_eq!(dl.status(), 400, "must return 400 when password is missing");

    // Download with WRONG password → should return 403
    let wrong = f
        .client
        .get(&format!("/f/{}/?password=wrongpass", token), None)
        .await;
    assert_eq!(wrong.status(), 403, "must return 403 for wrong password");

    // Download with CORRECT password via query parameter → should succeed
    let ok = f
        .client
        .get(&format!("/f/{}/?password=mypassword", token), None)
        .await;
    assert_eq!(ok.status(), 200, "should succeed with correct password");
    let content = ok.bytes().await.unwrap();
    assert_eq!(&content[..], b"secret data");

    // Download with CORRECT password via HTTP header → should also succeed
    let raw_client = reqwest::Client::builder().no_proxy().build().unwrap();
    let ok2 = raw_client
        .get(format!("{}/f/{}/", f.server.base_url, token))
        .header("X-Seafile-Sharelink-Password", "mypassword")
        .send()
        .await
        .unwrap();
    assert_eq!(ok2.status(), 200, "should succeed with header password");
}

/// H.3 — Share link with expiry (past timestamp)
#[tokio::test]
async fn test_share_link_expired() {
    let f = TestFixture::new().await;

    let up = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "temp.txt", b"temporary")
        .await;
    assert!(up.status().is_success());

    // Create share link with negative expire_days (past expiry)
    // expire_days=-1 → now - 86400, always expired
    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/temp.txt",
                "expire_days": -1,
            }),
        )
        .await;
    assert_eq!(resp.status(), 200, "create share link failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    // Download should fail because link is expired
    let dl = f.client.get(&format!("/f/{}/", token), None).await;
    assert_eq!(dl.status(), 404, "expired link must return 404");
}

/// H.4 — v2.1 DELETE share link by creator should succeed
#[tokio::test]
async fn test_share_link_delete_v21_own() {
    let f = TestFixture::new().await;

    // Upload a file to share
    let up = f
        .client
        .upload_file(
            &f.api_token,
            &f.repo_id,
            "/",
            "delete_test.txt",
            b"delete test",
        )
        .await;
    assert!(up.status().is_success(), "upload failed");

    // Create share link via v2.1
    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/delete_test.txt",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    // Confirm download works first
    let dl = f.client.get(&format!("/f/{}/", token), None).await;
    assert_eq!(dl.status(), 200, "download should work before delete");

    // Creator deletes via v2.1 — should succeed
    let del = f
        .client
        .delete(
            &format!("/api/v2.1/share-links/{}/", token),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(
        del.status(),
        200,
        "creator should be able to delete own link"
    );

    // Verify it's gone
    let dl = f.client.get(&format!("/f/{}/", token), None).await;
    assert_eq!(dl.status(), 404);
}

/// H.5 — v2.1 DELETE share link by other user should fail
#[tokio::test]
async fn test_share_link_delete_v21_other() {
    let f = TestFixture::new().await;
    let db = &*f.server.db;

    // Upload a file to share
    let up = f
        .client
        .upload_file(
            &f.api_token,
            &f.repo_id,
            "/",
            "other_test.txt",
            b"other test",
        )
        .await;
    assert!(up.status().is_success(), "upload failed");

    // Create a second user
    let _user2_id = create_test_user(db, "user2@test.com", "password2").await;
    let resp2 = f.client.login("user2@test.com", "password2").await;
    assert_eq!(resp2.status(), 200);
    let token2_val: serde_json::Value = resp2.json().await.unwrap();
    let api_token2 = token2_val["token"].as_str().unwrap().to_string();

    // User1 creates a share link via v2.1
    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/other_test.txt",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    // User2 tries to delete via v2.1 — should fail (not their link)
    let del = f
        .client
        .delete(
            &format!("/api/v2.1/share-links/{}/", token),
            Some(&api_token2),
        )
        .await;
    assert_eq!(del.status(), 404, "other user's delete must return 404");

    // Verify link still exists (user1 can still download)
    let dl = f.client.get(&format!("/f/{}/", token), None).await;
    assert_eq!(dl.status(), 200, "link should still be valid");
}

/// H.6 — v1 API (api2/shared-links) DELETE by creator should succeed
#[tokio::test]
async fn test_share_link_delete_v1_own() {
    let f = TestFixture::new().await;

    // Upload a file
    let up = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "v1delete.txt", b"v1 test")
        .await;
    assert!(up.status().is_success(), "upload failed");

    // Use v2.1 API to create, then test v1 delete
    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/v1delete.txt",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200, "create share link via v2.1 failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();
    assert!(!token.is_empty());

    // Creator deletes via v1 API (api2/shared-links/{token})
    let del = f
        .client
        .delete(&format!("/api2/shared-links/{}", token), Some(&f.api_token))
        .await;
    assert_eq!(del.status(), 200, "v1 delete should succeed for creator");
}

/// H.7 — Encrypted repo blocks share link creation
#[tokio::test]
async fn test_share_link_encrypted_repo_blocked() {
    let f = TestFixture::new().await;

    // Create an encrypted repo
    let repo_id = uuid::Uuid::new_v4().to_string();
    let magic =
        nanofile_server::crypto::key_derivation::generate_magic(&repo_id, "testpass", 2, "")
            .unwrap();
    let random_key =
        nanofile_server::crypto::key_derivation::generate_random_key_for_repo("testpass", 2, "")
            .unwrap();

    let resp = f
        .client
        .create_encrypted_repo(&f.api_token, "enc-repo", &repo_id, &magic, &random_key, 2)
        .await;
    assert_eq!(resp.status(), 201, "create encrypted repo failed");

    // Try to create share link — should fail
    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": repo_id,
                "path": "/",
            }),
        )
        .await;
    assert_eq!(
        resp.status(),
        400,
        "must reject share link for encrypted repo"
    );
}

/// H.8 — Invalid token returns 404
#[tokio::test]
async fn test_share_link_invalid_token() {
    let f = TestFixture::new().await;
    let dl = f.client.get("/f/nonexistent-token-xxx/", None).await;
    assert_eq!(dl.status(), 404);
}

/// H.9 — Share link list returns expected fields
#[tokio::test]
async fn test_share_link_list_response_fields() {
    let f = TestFixture::new().await;

    // Create a share link
    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);

    // List share links and verify response fields
    let list = f
        .client
        .get("/api/v2.1/share-links/", Some(&f.api_token))
        .await;
    assert_eq!(list.status(), 200);
    let body: serde_json::Value = list.json().await.unwrap();
    let links = body["share_link_list"].as_array().unwrap();
    assert!(!links.is_empty(), "should have at least one share link");

    let link = &links[0];
    assert!(
        !link["token"].as_str().unwrap_or("").is_empty(),
        "token missing"
    );
    assert!(
        link["link"].as_str().unwrap_or("").starts_with("/f/"),
        "link should start with /f/"
    );
    assert!(
        link["has_password"].is_boolean(),
        "has_password should be boolean"
    );
    assert!(link["s_type"].is_string(), "s_type should be string");
    assert_eq!(link["view_cnt"], 0, "view_cnt should be 0");
    assert!(
        link.get("expire_at").is_some(),
        "expire_at should be present"
    );
}

/// H.10 — Upload link creation with password
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

    // List upload links and verify has_password field
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
