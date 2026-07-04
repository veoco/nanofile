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

    // Download via shared link with ?dl=1 (no auth, no password)
    let dl = f.client.get(&format!("/f/{}/?dl=1", token), None).await;
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

    // Download WITHOUT password → should return 200 (HTML password form)
    let dl = f.client.get(&format!("/f/{}/", token), None).await;
    assert_eq!(dl.status(), 200, "must return 200 with password form");
    let body = dl.text().await.unwrap();
    assert!(
        body.contains("Password Required"),
        "should show password form"
    );

    // Download with WRONG password → should return 200 (password form with error)
    let wrong = f
        .client
        .get(&format!("/f/{}/?password=wrongpass", token), None)
        .await;
    assert_eq!(wrong.status(), 200, "must return 200 with error");
    let body = wrong.text().await.unwrap();
    assert!(
        body.contains("Incorrect password"),
        "should show error message"
    );

    // Download with CORRECT password via query parameter (?dl=1) → should succeed
    let ok = f
        .client
        .get(&format!("/f/{}/?dl=1&password=mypassword", token), None)
        .await;
    assert_eq!(ok.status(), 200, "should succeed with correct password");
    let content = ok.bytes().await.unwrap();
    assert_eq!(&content[..], b"secret data");

    // Download with CORRECT password via HTTP header (?dl=1) → should also succeed
    let raw_client = reqwest::Client::builder().no_proxy().build().unwrap();
    let ok2 = raw_client
        .get(format!("{}/f/{}/?dl=1", f.server.base_url, token))
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
    let dl = f.client.get(&format!("/f/{}/?dl=1", token), None).await;
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
    let dl = f.client.get(&format!("/f/{}/?dl=1", token), None).await;
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
        server::crypto::key_derivation::generate_magic(&repo_id, "testpass", 2, "").unwrap();
    let random_key =
        server::crypto::key_derivation::generate_random_key_for_repo("testpass", 2, "").unwrap();

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

/// H.11 — PUT update password on share link
#[tokio::test]
async fn test_share_link_update_password() {
    let f = TestFixture::new().await;

    // Upload a file
    let up = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "pw_update.txt", b"pw update")
        .await;
    assert!(up.status().is_success());

    // Create share link with initial password
    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/pw_update.txt",
                "password": "oldpass",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    // Verify old password works
    let ok = f
        .client
        .get(&format!("/f/{}/?dl=1&password=oldpass", token), None)
        .await;
    assert_eq!(ok.status(), 200);

    // PUT update to new password
    let upd = f
        .client
        .put_json(
            &format!("/api/v2.1/share-links/{}/", token),
            Some(&f.api_token),
            &serde_json::json!({"password": "newpass"}),
        )
        .await;
    assert_eq!(upd.status(), 200, "update password should succeed");

    // Old password should no longer work
    let old = f
        .client
        .get(&format!("/f/{}/?dl=1&password=oldpass", token), None)
        .await;
    assert_eq!(old.status(), 200, "wrong password shows form");
    let body_text = old.text().await.unwrap();
    assert!(
        body_text.contains("Incorrect password"),
        "old password should be rejected"
    );

    // New password should work
    let ok = f
        .client
        .get(&format!("/f/{}/?dl=1&password=newpass", token), None)
        .await;
    assert_eq!(ok.status(), 200);
    let content = ok.bytes().await.unwrap();
    assert_eq!(&content[..], b"pw update");
}

/// H.12 — PUT clear password on share link (set to null)
#[tokio::test]
async fn test_share_link_clear_password() {
    let f = TestFixture::new().await;

    let up = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "pw_clear.txt", b"pw clear")
        .await;
    assert!(up.status().is_success());

    // Create share link with password
    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/pw_clear.txt",
                "password": "secret",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    // PUT clear password (null)
    let upd = f
        .client
        .put_json(
            &format!("/api/v2.1/share-links/{}/", token),
            Some(&f.api_token),
            &serde_json::json!({"password": null}),
        )
        .await;
    assert_eq!(upd.status(), 200, "clear password should succeed");

    // Should now be accessible without password
    let ok = f.client.get(&format!("/f/{}/?dl=1", token), None).await;
    assert_eq!(ok.status(), 200);
    let content = ok.bytes().await.unwrap();
    assert_eq!(&content[..], b"pw clear");
}

/// H.13 — PUT update expiry on share link
#[tokio::test]
async fn test_share_link_update_expiry() {
    let f = TestFixture::new().await;

    let up = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "exp_test.txt", b"exp test")
        .await;
    assert!(up.status().is_success());

    // Create share link with 1-day expiry
    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/exp_test.txt",
                "expire_days": 1,
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    // Verify accessible now
    let ok = f.client.get(&format!("/f/{}/?dl=1", token), None).await;
    assert_eq!(ok.status(), 200);

    // PUT clear expiry (null → never expires)
    let upd = f
        .client
        .put_json(
            &format!("/api/v2.1/share-links/{}/", token),
            Some(&f.api_token),
            &serde_json::json!({"expire_days": null}),
        )
        .await;
    assert_eq!(upd.status(), 200, "clear expiry should succeed");
}

/// H.14 — PUT update share link by non-owner returns 404
#[tokio::test]
async fn test_share_link_update_other_fails() {
    let f = TestFixture::new().await;
    let db = &*f.server.db;

    let up = f
        .client
        .upload_file(
            &f.api_token,
            &f.repo_id,
            "/",
            "other_update.txt",
            b"other update",
        )
        .await;
    assert!(up.status().is_success());

    // Create share link as user1
    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/other_update.txt",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap().to_string();

    // Create a second user
    let _user2_id = common::create_test_user(db, "user2-upd@test.com", "password2").await;
    let resp2 = f.client.login("user2-upd@test.com", "password2").await;
    assert_eq!(resp2.status(), 200);
    let token2_val: serde_json::Value = resp2.json().await.unwrap();
    let api_token2 = token2_val["token"].as_str().unwrap().to_string();

    // User2 tries to update user1's link
    let upd = f
        .client
        .put_json(
            &format!("/api/v2.1/share-links/{}/", token),
            Some(&api_token2),
            &serde_json::json!({"expire_days": 7}),
        )
        .await;
    assert_eq!(upd.status(), 404, "non-owner update must return 404");
}

/// H.15 — PUT update non-existent token returns 404
#[tokio::test]
async fn test_share_link_update_nonexistent() {
    let f = TestFixture::new().await;

    let upd = f
        .client
        .put_json(
            "/api/v2.1/share-links/nonexistent-token/",
            Some(&f.api_token),
            &serde_json::json!({"expire_days": 7}),
        )
        .await;
    assert_eq!(upd.status(), 404, "non-existent token must return 404");
}
