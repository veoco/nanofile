mod common;

use common::TestFixture;
use server::crypto::key_derivation;

/// Pre-compute encrypted repo params using the Rust crypto module directly.
fn make_encrypted_params(repo_id: &str, password: &str) -> (String, String) {
    let magic = key_derivation::generate_magic(repo_id, password, 2, "").unwrap();
    let random_key = key_derivation::generate_random_key_for_repo(password, 2, "").unwrap();
    (magic, random_key)
}

/// Create an encrypted repo and return its repo_id.
async fn create_encrypted_repo(f: &TestFixture, name: &str, password: &str) -> String {
    let repo_id = uuid::Uuid::new_v4().to_string();
    let (magic, random_key) = make_encrypted_params(&repo_id, password);
    let resp = f
        .client
        .create_encrypted_repo(&f.api_token, name, &repo_id, &magic, &random_key, 2)
        .await;
    assert_eq!(
        resp.status(),
        201,
        "encrypted repo creation failed: {}",
        resp.text().await.unwrap_or_default()
    );
    repo_id
}

// ─── Phase 2: Encrypted Repo API ─────────────────────────────────────────────

#[tokio::test]
async fn test_create_encrypted_repo_client_side() {
    let f = TestFixture::new().await;

    let repo_id = uuid::Uuid::new_v4().to_string();
    let (magic, random_key) = make_encrypted_params(&repo_id, "test-password");

    let resp = f
        .client
        .create_encrypted_repo(&f.api_token, "enc-lib", &repo_id, &magic, &random_key, 2)
        .await;
    assert_eq!(resp.status(), 201, "create encrypted repo failed");

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["encrypted"], true);
    assert_eq!(body["enc_version"], 2);
    assert_eq!(body["magic"], magic);
    assert_eq!(body["random_key"], random_key);
}

#[tokio::test]
async fn test_list_repos_shows_encrypted() {
    let f = TestFixture::new().await;

    let enc_repo_id = create_encrypted_repo(&f, "enc-lib", "test-password").await;

    let resp = f.client.list_repos(&f.api_token).await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let repos = body.as_array().unwrap();
    let enc_repo = repos.iter().find(|r| r["id"] == enc_repo_id).unwrap();
    assert_eq!(enc_repo["encrypted"], true);
    assert_eq!(enc_repo["enc_version"], 2);
    assert!(enc_repo["magic"].as_str().is_some());
}

// ─── Password Set/Check ───────────────────────────────────────────────────────

#[tokio::test]
async fn test_set_password_v21_success() {
    let f = TestFixture::new().await;
    let enc_repo_id = create_encrypted_repo(&f, "enc-lib", "test-password").await;

    let resp = f
        .client
        .set_repo_password_v21(&f.api_token, &enc_repo_id, "test-password")
        .await;
    assert_eq!(resp.status(), 200, "set password failed");

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["success"], true);
}

#[tokio::test]
async fn test_set_password_wrong_password() {
    let f = TestFixture::new().await;
    let enc_repo_id = create_encrypted_repo(&f, "enc-lib", "test-password").await;

    let resp = f
        .client
        .set_repo_password_v21(&f.api_token, &enc_repo_id, "wrong-password")
        .await;
    assert_eq!(resp.status(), 440, "should fail with 440");
}

#[tokio::test]
async fn test_set_password_non_encrypted() {
    let f = TestFixture::new().await;
    // f.repo_id is a regular (non-encrypted) repo

    let resp = f
        .client
        .set_repo_password_v21(&f.api_token, &f.repo_id, "some-password")
        .await;
    assert_eq!(resp.status(), 400, "should fail with 400");
}

#[tokio::test]
async fn test_set_password_v2_success() {
    let f = TestFixture::new().await;
    let enc_repo_id = create_encrypted_repo(&f, "enc-lib", "test-password").await;

    let resp = f
        .client
        .set_repo_password_v2(&f.api_token, &enc_repo_id, "test-password")
        .await;
    assert_eq!(resp.status(), 200, "v2 set password failed");
}

#[tokio::test]
async fn test_check_password_v2_correct() {
    let f = TestFixture::new().await;
    let password = "test-password";
    let enc_repo_id = create_encrypted_repo(&f, "enc-lib", password).await;

    // Re-compute the magic using the actual repo_id from the DB
    let (magic, _) = make_encrypted_params(&enc_repo_id, password);

    let resp = f
        .client
        .check_repo_password_v2(&f.api_token, &enc_repo_id, &magic)
        .await;
    assert_eq!(resp.status(), 200, "check password failed");
}

#[tokio::test]
async fn test_check_password_v2_wrong() {
    let f = TestFixture::new().await;
    let enc_repo_id = create_encrypted_repo(&f, "enc-lib", "test-password").await;

    let resp = f
        .client
        .check_repo_password_v2(&f.api_token, &enc_repo_id, "00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000")
        .await;
    assert_eq!(resp.status(), 441, "should fail with 441");
}

// ─── Password Change ──────────────────────────────────────────────────────────

#[tokio::test]
async fn test_change_password_success() {
    let f = TestFixture::new().await;
    let enc_repo_id = create_encrypted_repo(&f, "enc-lib", "old-password").await;

    let resp = f
        .client
        .change_repo_password(&f.api_token, &enc_repo_id, "old-password", "new-password")
        .await;
    assert_eq!(
        resp.status(),
        200,
        "change password failed: {}",
        resp.text().await.unwrap_or_default()
    );

    // Verify new password works
    let resp = f
        .client
        .set_repo_password_v21(&f.api_token, &enc_repo_id, "new-password")
        .await;
    assert_eq!(resp.status(), 200, "new password should work");

    // Old password should fail
    let resp = f
        .client
        .set_repo_password_v21(&f.api_token, &enc_repo_id, "old-password")
        .await;
    assert_eq!(resp.status(), 440, "old password should fail");
}

// ─── Repo Info ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_get_encrypted_repo_info() {
    let f = TestFixture::new().await;
    let enc_repo_id = create_encrypted_repo(&f, "enc-lib", "test-password").await;

    let resp = f.client.get_repo(&f.api_token, &enc_repo_id).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["encrypted"], true);
    assert_eq!(body["enc_version"], 2);
    assert!(body["magic"].as_str().is_some());
    assert!(body["random_key"].as_str().is_some());
}

#[tokio::test]
async fn test_download_info_encrypted_repo() {
    let f = TestFixture::new().await;
    let enc_repo_id = create_encrypted_repo(&f, "enc-lib", "test-password").await;

    let resp = f.client.download_info(&f.api_token, &enc_repo_id).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["encrypted"], "true");
    assert!(body["magic"].as_str().is_some());
    assert!(body["random_key"].as_str().is_some());
}

// ─── Restrictions ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_share_link_blocked_for_encrypted_repo() {
    let f = TestFixture::new().await;
    let enc_repo_id = create_encrypted_repo(&f, "enc-lib", "test-password").await;

    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &serde_json::json!({"repo_id": enc_repo_id, "path": "/"}),
        )
        .await;
    assert_eq!(
        resp.status(),
        400,
        "share link for encrypted repo should be blocked, got: {}",
        resp.status()
    );

    let body: serde_json::Value = resp.json().await.unwrap_or_default();
    let err_msg = body["error_msg"].as_str().unwrap_or("");
    assert!(
        err_msg.contains("encrypted"),
        "error should mention encryption: {err_msg}"
    );
}

#[tokio::test]
async fn test_file_detail_on_encrypted_repo() {
    let f = TestFixture::new().await;
    let enc_repo_id = create_encrypted_repo(&f, "enc-lib", "test-password").await;

    // Set password and upload a file
    f.client
        .set_repo_password_v21(&f.api_token, &enc_repo_id, "test-password")
        .await;
    f.client
        .upload_file(&f.api_token, &enc_repo_id, "/", "doc.txt", b"metadata")
        .await;

    // File detail (metadata only, no decryption needed)
    let resp = f
        .client
        .get(
            &format!("/api2/repos/{enc_repo_id}/file/detail/?p=/doc.txt"),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200, "file detail should work");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "doc.txt");
    assert!(body["size"].as_i64().unwrap_or(0) > 0);
}
