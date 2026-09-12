mod common;

use common::{TestFixture, create_test_user};
use infra::crypto::key_derivation;

/// Pre-compute encrypted repo params using the Rust crypto module directly.
fn make_encrypted_params(repo_id: &str, password: &str) -> (String, String) {
    let magic = key_derivation::generate_magic(repo_id, password, 2, "").unwrap();
    let random_key = key_derivation::generate_random_key_for_repo(password, 2, "").unwrap();
    (magic, random_key)
}

/// Whether `token`'s session currently holds a cached decryption key for the
/// library, as reported by the `check-password` operation. This is the
/// client-visible view of the server-side key cache.
async fn cached_key_is_set(f: &TestFixture, token: &str, repo_id: &str) -> bool {
    let resp = f
        .client
        .put_json(
            &format!("/api/v2.1/repos/{repo_id}/set-password/?operation=check-password"),
            Some(token),
            &serde_json::json!({}),
        )
        .await;
    assert_eq!(resp.status(), 200, "check-password failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    body["is_set"].as_bool().unwrap_or(false)
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

/// Security: a read-only member must not be able to change an encrypted repo's
/// password (that would let them re-encrypt content affecting all members).
#[tokio::test]
async fn test_change_password_readonly_member_forbidden() {
    let f = TestFixture::new().await;
    let enc_repo_id = create_encrypted_repo(&f, "enc-lib", "old-password").await;

    // Second user shared as read-only.
    create_test_user(f.server.db.as_ref(), "ro@example.com", "password").await;
    let resp = f.client.login("ro@example.com", "password").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let b_token = body["token"].as_str().unwrap().to_string();

    let resp = f
        .client
        .post_json(
            &format!("/api2/beshared-repos/{enc_repo_id}/"),
            Some(&f.api_token),
            &serde_json::json!({
                "share_type": "personal",
                "user": "ro@example.com",
                "permission": "r"
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);

    // Read-only member must not be able to change the password, even knowing
    // the old one.
    let resp = f
        .client
        .change_repo_password(&b_token, &enc_repo_id, "old-password", "hacked-password")
        .await;
    assert_eq!(
        resp.status(),
        403,
        "read-only member must not change the encrypted repo password"
    );
}

// ==================== Security: encrypted-repo creation validation ====================

/// An encrypted repo with an unsupported enc_version (the server can only
/// operate 2/4) must be rejected.
#[tokio::test]
async fn test_create_encrypted_repo_rejects_unsupported_version() {
    let f = TestFixture::new().await;
    let repo_id = uuid::Uuid::new_v4().to_string();
    let (magic, random_key) = make_encrypted_params(&repo_id, "pw");

    let resp = f
        .client
        .create_encrypted_repo(&f.api_token, "bad-ver", &repo_id, &magic, &random_key, 1)
        .await;
    assert_eq!(resp.status(), 400, "enc_version 1 must be rejected");
}

/// A magic that isn't 64 hex chars must be rejected.
#[tokio::test]
async fn test_create_encrypted_repo_rejects_bad_magic() {
    let f = TestFixture::new().await;
    let repo_id = uuid::Uuid::new_v4().to_string();
    let (_, random_key) = make_encrypted_params(&repo_id, "pw");

    let resp = f
        .client
        .create_encrypted_repo(
            &f.api_token,
            "bad-magic",
            &repo_id,
            &"g".repeat(64),
            &random_key,
            2,
        )
        .await;
    assert_eq!(resp.status(), 400, "non-hex magic must be rejected");
}

/// A malformed random_key must be rejected.
#[tokio::test]
async fn test_create_encrypted_repo_rejects_bad_random_key() {
    let f = TestFixture::new().await;
    let repo_id = uuid::Uuid::new_v4().to_string();
    let (magic, _) = make_encrypted_params(&repo_id, "pw");

    let resp = f
        .client
        .create_encrypted_repo(
            &f.api_token,
            "bad-rk",
            &repo_id,
            &magic,
            "not-a-random-key",
            2,
        )
        .await;
    assert_eq!(resp.status(), 400, "malformed random_key must be rejected");
}

/// An encrypted repo without a magic must be rejected.
#[tokio::test]
async fn test_create_encrypted_repo_requires_magic() {
    let f = TestFixture::new().await;
    let repo_id = uuid::Uuid::new_v4().to_string();
    let (_, random_key) = make_encrypted_params(&repo_id, "pw");

    let resp = f
        .client
        .post_form(
            "/api2/repos/",
            Some(&f.api_token),
            &[
                ("name", "no-magic"),
                ("encrypted", "1"),
                ("enc_version", "2"),
                ("random_key", &random_key),
            ],
        )
        .await;
    assert_eq!(resp.status(), 400, "missing magic must be rejected");
}

/// A valid enc_version 4 repo (per-repo salt) is still accepted, and the salt
/// the client derived its keys from must be stored with the library.
#[tokio::test]
async fn test_create_encrypted_repo_accepts_v4() {
    let f = TestFixture::new().await;
    let repo_id = uuid::Uuid::new_v4().to_string();
    let salt = key_derivation::generate_repo_salt();
    let magic = key_derivation::generate_magic(&repo_id, "pw", 4, &salt).unwrap();
    let random_key = key_derivation::generate_random_key_for_repo("pw", 4, &salt).unwrap();

    let resp = f
        .client
        .create_encrypted_repo_with_params(
            &f.api_token,
            &common::client::EncRepoCreate {
                name: "v4-lib",
                repo_id: &repo_id,
                magic: &magic,
                random_key: &random_key,
                enc_version: 4,
                salt: Some(&salt),
            },
        )
        .await;
    assert_eq!(
        resp.status(),
        201,
        "enc_version 4 with valid keys should succeed, body={:?}",
        resp.text().await
    );

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["encrypted"], true);
    assert_eq!(body["enc_version"], 4);
    assert_eq!(
        body["salt"], salt,
        "the per-library salt must be stored, not discarded"
    );
}

/// A v4 library without a salt cannot be decrypted by the client that created
/// it, so the request must be rejected instead of silently storing an empty
/// salt (which is what used to happen).
#[tokio::test]
async fn test_create_encrypted_repo_v4_requires_salt() {
    let f = TestFixture::new().await;
    let repo_id = uuid::Uuid::new_v4().to_string();
    let salt = key_derivation::generate_repo_salt();
    let magic = key_derivation::generate_magic(&repo_id, "pw", 4, &salt).unwrap();
    let random_key = key_derivation::generate_random_key_for_repo("pw", 4, &salt).unwrap();

    let resp = f
        .client
        .create_encrypted_repo(&f.api_token, "v4-no-salt", &repo_id, &magic, &random_key, 4)
        .await;
    assert_eq!(resp.status(), 400, "v4 without salt must be rejected");
}

// ─── Brute-force metering (security) ─────────────────────────────────────────

/// Failed library-password checks are metered per (user, repo), and the budget
/// is shared by **every** endpoint that verifies the password or its magic.
///
/// The library KDF iteration count is fixed at 1000 by the Seafile wire
/// protocol, so this limiter is the only control against online guessing. Two
/// of the verification endpoints used to be unmetered, which made the cap
/// irrelevant: an attacker simply switched endpoints.
#[tokio::test]
async fn library_password_guessing_is_metered_across_every_endpoint() {
    /// Must match `repo_password_max_per_hour` in `server/tests/common/mod.rs`.
    const LIMIT: u32 = 10;

    let f = TestFixture::new().await;
    let enc_repo_id = create_encrypted_repo(&f, "enc-lib", "test-password").await;
    let (magic, _) = make_encrypted_params(&enc_repo_id, "test-password");

    // Spend the budget through the v2.1 endpoint (the one the Android client
    // calls with its cached password before every download).
    for attempt in 0..LIMIT {
        let resp = f
            .client
            .set_repo_password_v21(&f.api_token, &enc_repo_id, "wrong-password")
            .await;
        assert_eq!(
            resp.status(),
            440,
            "guess {attempt} must be verified, not throttled yet"
        );
    }

    // The next guess is rejected before any KDF work.
    let resp = f
        .client
        .set_repo_password_v21(&f.api_token, &enc_repo_id, "wrong-password")
        .await;
    assert_eq!(resp.status(), 429, "v2.1 set-password must be metered");

    // Switching endpoints must not hand the attacker a fresh budget.
    let resp = f
        .client
        .set_repo_password_v2(&f.api_token, &enc_repo_id, "wrong-password")
        .await;
    assert_eq!(resp.status(), 429, "?op=setpassword shares the meter");

    let resp = f
        .client
        .check_repo_password_v2(&f.api_token, &enc_repo_id, &magic)
        .await;
    assert_eq!(resp.status(), 429, "?op=checkpassword shares the meter");

    // The change-password operation verifies the old password as well.
    let resp = f
        .client
        .change_repo_password(&f.api_token, &enc_repo_id, "wrong-password", "new-password")
        .await;
    assert_eq!(resp.status(), 429, "change-password shares the meter");
}

/// Only failures may consume budget: a client re-submitting its own correct
/// password (Android does this before every download) must never be throttled,
/// and a success clears previously recorded failures.
#[tokio::test]
async fn correct_library_password_never_consumes_the_failure_budget() {
    /// Must match `repo_password_max_per_hour` in `server/tests/common/mod.rs`.
    const LIMIT: u32 = 10;

    let f = TestFixture::new().await;
    let enc_repo_id = create_encrypted_repo(&f, "enc-lib", "test-password").await;

    // More successful submissions than the whole failure budget.
    for _ in 0..(LIMIT + 5) {
        let resp = f
            .client
            .set_repo_password_v21(&f.api_token, &enc_repo_id, "test-password")
            .await;
        assert_eq!(
            resp.status(),
            200,
            "a correct password must always be accepted"
        );
    }

    // A success clears recorded failures: three wrong guesses followed by a
    // success leave a full budget again.
    for _ in 0..3 {
        let resp = f
            .client
            .set_repo_password_v21(&f.api_token, &enc_repo_id, "wrong-password")
            .await;
        assert_eq!(resp.status(), 440);
    }
    let resp = f
        .client
        .set_repo_password_v21(&f.api_token, &enc_repo_id, "test-password")
        .await;
    assert_eq!(resp.status(), 200, "success must be accepted");
    for attempt in 0..LIMIT {
        let resp = f
            .client
            .set_repo_password_v21(&f.api_token, &enc_repo_id, "wrong-password")
            .await;
        assert_eq!(
            resp.status(),
            440,
            "guess {attempt} must see a budget cleared by the earlier success"
        );
    }
    let resp = f
        .client
        .set_repo_password_v21(&f.api_token, &enc_repo_id, "wrong-password")
        .await;
    assert_eq!(resp.status(), 429);
}

/// Rotating a library password must invalidate **every** cached decryption key,
/// not just the caller's.
///
/// Otherwise a member who knew the old password keeps decrypting the library
/// from the in-memory key cache for up to its TTL (1 hour), so revoking a
/// password never really takes effect for them.
#[tokio::test]
async fn test_password_rotation_evicts_every_cached_key() {
    let f = TestFixture::new().await;
    let enc_repo_id = create_encrypted_repo(&f, "enc-lib", "old-password").await;

    // A second member with write access.
    create_test_user(f.server.db.as_ref(), "member@example.com", "password").await;
    let resp = f
        .client
        .post_json(
            &format!("/api2/beshared-repos/{}/", enc_repo_id),
            Some(&f.api_token),
            &serde_json::json!({
                "share_type": "personal",
                "user": "member@example.com",
                "permission": "rw",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200, "sharing the library failed");
    let resp = f.client.login("member@example.com", "password").await;
    let member_token = resp.json::<serde_json::Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    // The member caches the key by supplying the correct library password.
    let resp = f
        .client
        .set_repo_password_v21(&member_token, &enc_repo_id, "old-password")
        .await;
    assert_eq!(resp.status(), 200);
    assert!(
        cached_key_is_set(&f, &member_token, &enc_repo_id).await,
        "the member must hold a cached key after supplying the password"
    );

    // The owner rotates the password.
    let resp = f
        .client
        .change_repo_password(&f.api_token, &enc_repo_id, "old-password", "new-password")
        .await;
    assert_eq!(resp.status(), 200, "rotation failed");

    // Every other session's cached key is gone.
    assert!(
        !cached_key_is_set(&f, &member_token, &enc_repo_id).await,
        "rotation must evict other members' cached decryption keys"
    );

    // And the new password is the only one that works for the member.
    assert_eq!(
        f.client
            .set_repo_password_v21(&member_token, &enc_repo_id, "new-password")
            .await
            .status(),
        200
    );
    assert_eq!(
        f.client
            .set_repo_password_v21(&member_token, &enc_repo_id, "old-password")
            .await
            .status(),
        440
    );
}
