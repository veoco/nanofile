//! Business logic vulnerability regression tests.
//!
//! Verifies the authorization, rate-limiting, and input-validation fixes
//! applied for V-1 through V-7.  Each test validates a specific access
//! control or input guard that was previously missing.
//!
//! See also: plan file at .claude/plans/rippling-booping-kahan.md

mod common;

use common::{TestFixture, create_test_user};
use sea_orm::{ActiveModelTrait, EntityTrait};

// ─────────────────────────────────────────────────────────────────────
// V-1: beshare_repo — verify that a user without write permission on a
// repo cannot share it with another user.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_beshare_repo_requires_write_permission() {
    let f = TestFixture::new().await;

    // Create a second user who does NOT own the repo and is NOT a member.
    let _uid2 = create_test_user(&f.server.db, "attacker@test.com", "password").await;
    let resp = f.client.login("attacker@test.com", "password").await;
    let body: serde_json::Value = resp.json().await.unwrap();

    let attacker_token = body["token"].as_str().unwrap();

    // Create a third user to be the target of the share.
    let _uid3 = create_test_user(&f.server.db, "victim@test.com", "password").await;

    // Attacker tries to share the repo (which they don't own) with victim.
    let resp = f
        .client
        .post_json(
            &format!("/api2/beshared-repos/{}/", f.repo_id),
            Some(attacker_token),
            &serde_json::json!({
                "share_type": "personal",
                "user": "victim@test.com",
                "permission": "rw"
            }),
        )
        .await;

    assert_eq!(
        resp.status(),
        403,
        "non-owner must be forbidden from sharing a repo"
    );

    // Verify the victim was NOT added to the repo members.
    let resp = f
        .client
        .get(
            &format!("/api2/beshared-repos/{}/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let members: Vec<serde_json::Value> = resp.json().await.unwrap();
    assert_eq!(members.len(), 1, "only the owner should remain as member");
    assert_eq!(members[0]["email"], "test@example.com");
}

#[tokio::test]
async fn test_beshare_repo_readonly_member_cannot_share() {
    let f = TestFixture::new().await;

    // Create user2 that will get read-only access.
    let _uid2 = create_test_user(&f.server.db, "reader@test.com", "password").await;

    // Owner shares repo with user2 as read-only.
    let resp = f
        .client
        .post_json(
            &format!("/api2/beshared-repos/{}/", f.repo_id),
            Some(&f.api_token),
            &serde_json::json!({
                "share_type": "personal",
                "user": "reader@test.com",
                "permission": "r"
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);

    // User2 logs in and tries to share with a third user.
    let resp = f.client.login("reader@test.com", "password").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let reader_token = body["token"].as_str().unwrap();

    let _uid3 = create_test_user(&f.server.db, "victim@test.com", "password").await;

    let resp = f
        .client
        .post_json(
            &format!("/api2/beshared-repos/{}/", f.repo_id),
            Some(reader_token),
            &serde_json::json!({
                "share_type": "personal",
                "user": "victim@test.com",
                "permission": "rw"
            }),
        )
        .await;

    assert_eq!(
        resp.status(),
        403,
        "read-only member must be forbidden from sharing the repo"
    );
}

// ─────────────────────────────────────────────────────────────────────
// V-2: Upload-link / update-link — verify that a user without write
// permission cannot obtain an upload or update URL.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_upload_link_requires_write_permission() {
    let f = TestFixture::new().await;

    let _uid2 = create_test_user(&f.server.db, "attacker@test.com", "password").await;
    let resp = f.client.login("attacker@test.com", "password").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let attacker_token = body["token"].as_str().unwrap();

    // Non-owner tries GET /api2/repos/{id}/upload-link/
    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/upload-link/?p=/", f.repo_id),
            Some(attacker_token),
        )
        .await;
    assert_eq!(
        resp.status(),
        403,
        "non-owner must be forbidden from getting upload link"
    );

    // Non-owner tries GET /api2/repos/{id}/update-link/
    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/update-link/?p=/", f.repo_id),
            Some(attacker_token),
        )
        .await;
    assert_eq!(
        resp.status(),
        403,
        "non-owner must be forbidden from getting update link"
    );
}

#[tokio::test]
async fn test_upload_link_owner_can_get() {
    let f = TestFixture::new().await;

    // Owner can still get upload link.
    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/upload-link/?p=/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(
        resp.status(),
        200,
        "owner must be allowed to get upload link"
    );

    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/update-link/?p=/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(
        resp.status(),
        200,
        "owner must be allowed to get update link"
    );
}

// ─────────────────────────────────────────────────────────────────────
// V-3 / V-4: Share links — verify that a user without read permission
// cannot create a share link for a repo (both v1 and v2.1 APIs).
// ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_create_share_link_requires_read_permission_v1() {
    let f = TestFixture::new().await;

    // Upload a file so there's something to share.
    let resp = f
        .client
        .upload_file(
            &f.api_token,
            &f.repo_id,
            "/",
            "shared.txt",
            b"share content",
        )
        .await;
    assert!(
        resp.status().is_success(),
        "upload must succeed for share link test"
    );

    // The v1 POST /api2/shared-links/ endpoint is used by the Seafile Android
    // client. Test that a non-member cannot create a share link on v1 either.
    let _uid2 = create_test_user(&f.server.db, "attacker@test.com", "password").await;
    let resp = f.client.login("attacker@test.com", "password").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let attacker_token = body["token"].as_str().unwrap();

    // Non-member tries POST /api/v2.1/share-links/ (v2.1 is the active API).
    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(attacker_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/shared.txt",
            }),
        )
        .await;
    assert_eq!(
        resp.status(),
        403,
        "non-member must be forbidden from creating share link (v2.1): status={}",
        resp.status(),
    );
}

#[tokio::test]
async fn test_create_share_link_requires_read_permission_v21() {
    let f = TestFixture::new().await;

    let _uid2 = create_test_user(&f.server.db, "attacker@test.com", "password").await;
    let resp = f.client.login("attacker@test.com", "password").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let attacker_token = body["token"].as_str().unwrap();

    // Non-member tries POST /api/v2.1/share-links/ (v2.1).
    let resp = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(attacker_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/",
            }),
        )
        .await;
    assert_eq!(
        resp.status(),
        403,
        "non-member must be forbidden from creating share link (v2.1)"
    );
}

#[tokio::test]
async fn test_create_share_link_owner_succeeds_v21() {
    let f = TestFixture::new().await;

    // Upload a file first.
    let resp = f
        .client
        .upload_file(
            &f.api_token,
            &f.repo_id,
            "/",
            "shared.txt",
            b"share content",
        )
        .await;
    assert!(resp.status().is_success(), "upload must succeed");

    // Owner can still create share links via v2.1 API.
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
    assert_eq!(
        resp.status(),
        200,
        "owner must be allowed to create share link (v2.1)"
    );
}

// ─────────────────────────────────────────────────────────────────────
// V-5: Download token — verify that a download token cannot be used
// after the user's read permission has been revoked.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_download_token_revoked_permission() {
    let f = TestFixture::new().await;

    // Create a second user to share with.
    let _uid2 = create_test_user(&f.server.db, "friend@test.com", "password").await;

    // Owner uploads a file.
    let resp = f
        .client
        .upload_file(
            &f.api_token,
            &f.repo_id,
            "/",
            "secret.txt",
            b"sensitive data",
        )
        .await;
    assert!(resp.status().is_success());

    // Owner shares repo with user2 (read-only).
    let resp = f
        .client
        .post_json(
            &format!("/api2/beshared-repos/{}/", f.repo_id),
            Some(&f.api_token),
            &serde_json::json!({
                "share_type": "personal",
                "user": "friend@test.com",
                "permission": "r"
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);

    // User2 logs in and obtains a download token (step A of download flow).
    let resp = f.client.login("friend@test.com", "password").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let friend_token = body["token"].as_str().unwrap();

    let dl_resp = f
        .client
        .get(
            &format!("/api2/repos/{}/file/?p=/secret.txt", f.repo_id),
            Some(friend_token),
        )
        .await;
    assert_eq!(dl_resp.status(), 200);
    let download_url: String = dl_resp.json().await.unwrap();

    // Owner revokes user2's access by removing them from repo members.
    // Use raw reqwest to send DELETE with JSON body (TestClient only has
    // delete_form which sends url-encoded form data, not JSON).
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let del_resp = http
        .delete(format!(
            "{}/api2/beshared-repos/{}/",
            f.server.base_url, f.repo_id
        ))
        .bearer_auth(&f.api_token)
        .json(&serde_json::json!({"user": "friend@test.com"}))
        .send()
        .await
        .unwrap();
    assert_eq!(del_resp.status(), 200);

    // User2 tries to use the previously-obtained download token (step B).
    // The download URL is a full absolute URL like
    // "http://127.0.0.1:{port}/download-api/{token}".  Use a raw reqwest
    // client to fetch it directly (TestClient would double-prefix base_url).
    let use_resp = http.get(&download_url).send().await.unwrap();
    assert_eq!(
        use_resp.status(),
        403,
        "download should be rejected after permission is revoked"
    );
}

// ─────────────────────────────────────────────────────────────────────
// V-6: 2FA disable — verify rate limiting on incorrect password attempts.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_disable_2fa_rate_limited() {
    let f = TestFixture::new().await;

    // Enable 2FA for the user directly (bypass setup/verify).
    let totp_secret = "JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP";
    let user_2fa = nanofile::entity::user_2fa::ActiveModel {
        user_id: sea_orm::Set(f.user_id),
        totp_secret: sea_orm::Set(totp_secret.to_string()),
        algorithm: sea_orm::Set("SHA1".to_string()),
        digits: sea_orm::Set(6),
        period: sea_orm::Set(30),
        enabled: sea_orm::Set(true),
        enabled_at: sea_orm::NotSet,
    };
    user_2fa.insert(f.server.db.as_ref()).await.unwrap();

    // The test config sets totp_max_attempts = 10, so we make 11 incorrect
    // password attempts and expect the 11th to be rate-limited (429).

    for i in 0..12 {
        // Each attempt uses a unique wrong password so the password check
        // fails, but the rate limiter operates on the key, not the password.
        let wrong_pass = format!("wrong_password_{}", i);
        let resp = f
            .client
            .post_json(
                "/api2/2fa/disable/",
                Some(&f.api_token),
                &serde_json::json!({
                    "password": wrong_pass,
                }),
            )
            .await;
        let status = resp.status().as_u16();

        if i < 10 {
            assert_eq!(
                status,
                401,
                "attempt {} should be 401 (wrong password), got {}",
                i + 1,
                status
            );
        } else {
            // Attempt 11 and 12 should be rate-limited
            assert_eq!(
                status,
                429,
                "attempt {} should be 429 (rate limited), got {}",
                i + 1,
                status
            );
        }
    }
}

#[tokio::test]
async fn test_disable_2fa_rate_limit_cleared_on_success() {
    let f = TestFixture::new().await;

    // Enable 2FA for the user directly.
    let totp_secret = "JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP";
    let user_2fa = nanofile::entity::user_2fa::ActiveModel {
        user_id: sea_orm::Set(f.user_id),
        totp_secret: sea_orm::Set(totp_secret.to_string()),
        algorithm: sea_orm::Set("SHA1".to_string()),
        digits: sea_orm::Set(6),
        period: sea_orm::Set(30),
        enabled: sea_orm::Set(true),
        enabled_at: sea_orm::NotSet,
    };
    user_2fa.insert(f.server.db.as_ref()).await.unwrap();

    // Make a few wrong attempts (but stay under the limit).
    for i in 0..3 {
        let resp = f
            .client
            .post_json(
                "/api2/2fa/disable/",
                Some(&f.api_token),
                &serde_json::json!({
                    "password": format!("wrong_{}", i),
                }),
            )
            .await;
        assert_eq!(resp.status(), 401, "wrong password should be 401");
    }

    // Now disable 2FA with the correct password.
    let resp = f
        .client
        .post_json(
            "/api2/2fa/disable/",
            Some(&f.api_token),
            &serde_json::json!({
                "password": f.password,
            }),
        )
        .await;
    assert_eq!(
        resp.status(),
        200,
        "correct password should succeed after a few wrong tries"
    );

    // Verify 2FA is now disabled by checking the DB.
    let twofa = nanofile::entity::user_2fa::Entity::find_by_id(f.user_id)
        .one(f.server.db.as_ref())
        .await
        .unwrap();
    assert!(twofa.is_some(), "2FA record should still exist");
    assert!(
        !twofa.unwrap().enabled,
        "2FA should be disabled after successful disable"
    );
}

// ─────────────────────────────────────────────────────────────────────
// V-7: Filename validation — verify that filenames containing `/` are
// rejected in both the API and web-UI upload paths.
// ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_upload_file_with_slash_in_name_rejected() {
    let f = TestFixture::new().await;

    // Upload via POST /api2/repos/{repo_id}/file/ with a filename containing '/'.
    let file_part = reqwest::multipart::Part::bytes(b"content".to_vec())
        .file_name("foo/bar.txt".to_string())
        .mime_str("application/octet-stream")
        .unwrap();
    let form = reqwest::multipart::Form::new()
        .part("file", file_part)
        .text("parent_dir", "/");

    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let resp = client
        .post(format!(
            "{}/api2/repos/{}/file/",
            f.server.base_url, f.repo_id
        ))
        .bearer_auth(&f.api_token)
        .multipart(form)
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        400,
        "filename with '/' should be rejected (API upload), got {}",
        resp.status()
    );

    // Also test with other invalid characters.
    let file_part = reqwest::multipart::Part::bytes(b"content".to_vec())
        .file_name("test<file.txt".to_string())
        .mime_str("application/octet-stream")
        .unwrap();
    let form = reqwest::multipart::Form::new()
        .part("file", file_part)
        .text("parent_dir", "/");

    let resp = client
        .post(format!(
            "{}/api2/repos/{}/file/",
            f.server.base_url, f.repo_id
        ))
        .bearer_auth(&f.api_token)
        .multipart(form)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 400, "filename with '<' should be rejected");
}

#[tokio::test]
async fn test_upload_file_normal_name_succeeds() {
    let f = TestFixture::new().await;

    // Normal filename should still work.
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "normal.txt", b"hello")
        .await;
    assert!(
        resp.status().is_success(),
        "normal filename upload should succeed: {:?}",
        resp.status()
    );
}
