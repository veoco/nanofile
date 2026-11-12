use crate::common;

use base::common::S_IFREG;
use common::{TestFixture, create_test_user, get_sync_token, sync_commit, sync_commit_unchecked};

const REG: i32 = S_IFREG;

#[tokio::test]
async fn test_lock_file_success() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "lock-me.txt", b"content")
        .await;
    assert!(resp.status().is_success());

    let resp = f
        .client
        .put_sync(
            &format!("/seafhttp/repo/{}/lock-file?p=/lock-me.txt", f.repo_id),
            &f.sync_token,
            vec![],
        )
        .await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_lock_and_unlock_file() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "file.txt", b"data")
        .await;
    assert!(resp.status().is_success());

    let resp = f
        .client
        .put_sync(
            &format!("/seafhttp/repo/{}/lock-file?p=/file.txt", f.repo_id),
            &f.sync_token,
            vec![],
        )
        .await;
    assert_eq!(resp.status(), 200);

    let resp = f
        .client
        .put_sync(
            &format!("/seafhttp/repo/{}/unlock-file?p=/file.txt", f.repo_id),
            &f.sync_token,
            vec![],
        )
        .await;
    assert_eq!(resp.status(), 200);
}

/// The desktop client locks through one endpoint and may unlock through the
/// other: its file browser uses the REST endpoint, while the sync daemon uses
/// the sync endpoint. Both address the same repo file, so either unlock must
/// release the lock. Regression: `/file.txt` and `file.txt` used to be
/// distinct rows, making the cross-endpoint unlock a silent no-op.
#[tokio::test]
async fn test_rest_unlock_releases_a_sync_lock() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "shared.txt", b"data")
        .await;
    assert!(resp.status().is_success());

    let resp = f
        .client
        .put_sync(
            &format!("/seafhttp/repo/{}/lock-file?p=/shared.txt", f.repo_id),
            &f.sync_token,
            vec![],
        )
        .await;
    assert_eq!(resp.status(), 200);

    let resp = f
        .client
        .put_form(
            &format!("/api2/repos/{}/file/?p=/shared.txt", f.repo_id),
            Some(&f.api_token),
            &[("operation", "unlock")],
        )
        .await;
    assert_eq!(resp.status(), 200);

    assert_locked_files_empty(&f).await;
}

/// The reverse direction, and the path the sync clients expect: a lock created
/// through the REST endpoint is listed repo-relative so the daemon's index
/// (`dir/file`, no leading slash) can match it.
#[tokio::test]
async fn test_sync_unlock_releases_a_rest_lock() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "shared.txt", b"data")
        .await;
    assert!(resp.status().is_success());

    let resp = f
        .client
        .put_form(
            &format!("/api2/repos/{}/file/?p=/shared.txt", f.repo_id),
            Some(&f.api_token),
            &[("operation", "lock")],
        )
        .await;
    assert_eq!(resp.status(), 200);

    let req_body = serde_json::json!([{
        "repo_id": f.repo_id,
        "token": f.sync_token,
        "ts": 0,
    }]);
    let resp = f
        .client
        .post_sync_json("/seafhttp/repo/locked-files", &f.sync_token, &req_body)
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let entry = &body.as_array().unwrap()[0];
    assert_eq!(entry["locked_files"][0]["path"], "shared.txt");
    assert_eq!(entry["locked_files"][0]["by_me"], 1);

    let resp = f
        .client
        .put_sync(
            &format!("/seafhttp/repo/{}/unlock-file?p=shared.txt", f.repo_id),
            &f.sync_token,
            vec![],
        )
        .await;
    assert_eq!(resp.status(), 200);

    assert_locked_files_empty(&f).await;
}

/// The desktop client's file browser chooses between "Lock" and "Unlock"
/// from the listing's `is_locked`/`locked_by_me`. Without those fields it can
/// only offer "Lock" again, so an unlock from its UI never reaches the server.
#[tokio::test]
async fn test_listing_exposes_lock_state_for_the_desktop_client() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "ui.txt", b"data")
        .await;
    assert!(resp.status().is_success());

    let resp = f
        .client
        .put_form(
            &format!("/api2/repos/{}/file/?p=/ui.txt", f.repo_id),
            Some(&f.api_token),
            &[("operation", "lock")],
        )
        .await;
    assert_eq!(resp.status(), 200);

    let entry = listed_entry(&f, "ui.txt").await;
    assert_eq!(entry["is_locked"], true, "lock state must reach the client");
    assert_eq!(entry["locked_by_me"], true);
    assert_eq!(entry["lock_owner"], "test@example.com");
    assert!(
        entry["lock_time"].as_i64().unwrap() > 0,
        "the client formats lock_time, so it must be a unix timestamp: {entry}"
    );

    // The v2.1 listing the mobile clients and the web interface use carries
    // the same fields.
    let resp = f
        .client
        .get(
            &format!("/api/v2.1/repos/{}/dir/?p=/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let entry = body["dirent_list"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "ui.txt")
        .expect("ui.txt missing from the v2.1 listing");
    assert_eq!(entry["is_locked"], true);
    assert_eq!(entry["locked_by_me"], true);

    let resp = f
        .client
        .put_form(
            &format!("/api2/repos/{}/file/?p=/ui.txt", f.repo_id),
            Some(&f.api_token),
            &[("operation", "unlock")],
        )
        .await;
    assert_eq!(resp.status(), 200);

    let entry = listed_entry(&f, "ui.txt").await;
    assert!(
        entry.get("is_locked").is_none(),
        "an unlocked entry keeps the lock fields absent: {entry}"
    );
}

async fn listed_entry(f: &TestFixture, name: &str) -> serde_json::Value {
    let resp = f.client.list_dir(&f.api_token, &f.repo_id, "/").await;
    assert_eq!(resp.status(), 200);
    let body: Vec<serde_json::Value> = resp.json().await.unwrap();
    body.into_iter()
        .find(|e| e["name"] == name)
        .unwrap_or_else(|| panic!("{name} missing from the listing"))
}

async fn assert_locked_files_empty(f: &TestFixture) {
    let req_body = serde_json::json!([{
        "repo_id": f.repo_id,
        "token": f.sync_token,
        "ts": 0,
    }]);
    let resp = f
        .client
        .post_sync_json("/seafhttp/repo/locked-files", &f.sync_token, &req_body)
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body.as_array().unwrap()[0]["locked_files"]
            .as_array()
            .unwrap()
            .is_empty(),
        "unlock must remove the lock record: {body}"
    );
}

#[tokio::test]
async fn test_locked_files_list() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "locked.txt", b"data")
        .await;
    assert!(resp.status().is_success());

    let resp = f
        .client
        .put_sync(
            &format!("/seafhttp/repo/{}/lock-file?p=/locked.txt", f.repo_id),
            &f.sync_token,
            vec![],
        )
        .await;
    assert_eq!(resp.status(), 200);

    let req_body = serde_json::json!([{
        "repo_id": f.repo_id,
        "token": f.sync_token,
        "ts": 0,
    }]);
    let resp = f
        .client
        .post_sync_json("/seafhttp/repo/locked-files", &f.sync_token, &req_body)
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let list = body.as_array().unwrap();
    assert!(!list.is_empty(), "expected at least one locked file");

    let entry = &list[0];
    assert_eq!(entry["repo_id"], f.repo_id);
    // Locks are keyed the way the sync tree walk names files: repo-relative,
    // no leading slash, even when the lock arrived with an absolute path.
    assert_eq!(entry["locked_files"][0]["path"], "locked.txt");
}

#[tokio::test]
async fn test_locked_files_empty() {
    let f = TestFixture::new().await;

    let req_body = serde_json::json!([{
        "repo_id": f.repo_id,
        "token": f.sync_token,
        "ts": 0,
    }]);
    let resp = f
        .client
        .post_sync_json("/seafhttp/repo/locked-files", &f.sync_token, &req_body)
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert!(arr[0]["locked_files"].as_array().unwrap().is_empty());
}

/// Regression: locked-files must accept POST without Content-Type header.
/// seaf-daemon sends POST requests via curl without setting Content-Type.
#[tokio::test]
async fn test_locked_files_accepts_no_content_type() {
    let f = TestFixture::new().await;

    let body = serde_json::json!([{
        "repo_id": f.repo_id,
        "token": f.sync_token,
        "ts": 0,
    }]);

    let resp = f
        .client
        .post_sync_raw("/seafhttp/repo/locked-files", &f.sync_token, &body)
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().unwrap();
    assert_eq!(arr.len(), 1);
}

#[tokio::test]
async fn test_lock_file_requires_auth() {
    let server = common::TestServer::start().await;
    let client = server.client();
    let resp = client
        .put_sync("/seafhttp/repo/some-repo/lock-file?p=/f.txt", "", vec![])
        .await;
    assert!(resp.status() == 401 || resp.status() == 403);
}

/// Security: a read-only member must not be able to lock/unlock files.
#[tokio::test]
async fn test_lock_file_readonly_member_forbidden() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "lock-ro.txt", b"x")
        .await;
    assert!(resp.status().is_success());

    let ro_id = create_test_user(f.server.db.as_ref(), "ro@example.com", "password").await;
    let resp = f.client.login("ro@example.com", "password").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let b_token = body["token"].as_str().unwrap().to_string();

    common::add_repo_member(f.server.db.as_ref(), &f.repo_id, ro_id, "r").await;

    let b_sync = get_sync_token(&f.client, &b_token, &f.repo_id).await;

    let resp = f
        .client
        .put_sync(
            &format!("/seafhttp/repo/{}/lock-file?p=/lock-ro.txt", f.repo_id),
            &b_sync,
            vec![],
        )
        .await;
    assert_eq!(resp.status(), 403, "read-only member must not lock files");

    let resp = f
        .client
        .put_sync(
            &format!("/seafhttp/repo/{}/unlock-file?p=/lock-ro.txt", f.repo_id),
            &b_sync,
            vec![],
        )
        .await;
    assert_eq!(resp.status(), 403, "read-only member must not unlock files");
}

/// A second account with write access to the fixture's library.
async fn add_writer(f: &TestFixture, email: &str) -> String {
    let user_id = create_test_user(f.server.db.as_ref(), email, "password").await;
    common::add_repo_member(f.server.db.as_ref(), &f.repo_id, user_id, "rw").await;
    let resp = f.client.login(email, "password").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let api_token = body["token"].as_str().unwrap().to_string();
    get_sync_token(&f.client, &api_token, &f.repo_id).await
}

async fn lock_via_sync(f: &TestFixture, path: &str, sync_token: &str) {
    let resp = f
        .client
        .put_sync(
            &format!("/seafhttp/repo/{}/lock-file?p={path}", f.repo_id),
            sync_token,
            vec![],
        )
        .await;
    assert_eq!(resp.status(), 200, "lock {path} failed");
}

/// A foreign lock protects only the locked path: another user's edits to other
/// files must still be accepted. A whole-tree check would freeze every other
/// device in the library until the lock is released.
#[tokio::test]
async fn test_foreign_lock_does_not_block_unrelated_files() {
    let f = TestFixture::new().await;

    let base = sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[("locked.txt", b"v0", REG), ("other.txt", b"v0", REG)],
        &[],
        None,
    )
    .await;
    lock_via_sync(&f, "/locked.txt", &f.sync_token).await;

    let writer = add_writer(&f, "writer@example.com").await;
    sync_commit(
        &f.client,
        &writer,
        &f.repo_id,
        &[("locked.txt", b"v0", REG), ("other.txt", b"v1", REG)],
        &[],
        Some(&base),
    )
    .await;
}

/// Editing, deleting or renaming the locked path is still rejected for another
/// user. Those three operations are the ones a whole-tree check got right for
/// modifications and missed for deletes and renames.
#[tokio::test]
async fn test_foreign_lock_blocks_changes_to_the_locked_path() {
    let f = TestFixture::new().await;

    let base = sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[("locked.txt", b"v0", REG), ("other.txt", b"v0", REG)],
        &[],
        None,
    )
    .await;
    lock_via_sync(&f, "/locked.txt", &f.sync_token).await;

    let writer = add_writer(&f, "writer@example.com").await;

    // Edit.
    let (_, resp) = sync_commit_unchecked(
        &f.client,
        &writer,
        &f.repo_id,
        &[("locked.txt", b"v1", REG), ("other.txt", b"v0", REG)],
        &[],
        Some(&base),
    )
    .await;
    assert_eq!(resp.status(), 403);
    assert_eq!(
        resp.text().await.unwrap(),
        "File locked.txt is locked",
        "the official client parses this body"
    );

    // Delete.
    let (_, resp) = sync_commit_unchecked(
        &f.client,
        &writer,
        &f.repo_id,
        &[("other.txt", b"v0", REG)],
        &[],
        Some(&base),
    )
    .await;
    assert_eq!(
        resp.status(),
        403,
        "deleting a locked file must be rejected"
    );

    // Rename (same content, so the diff pairs the old and new path).
    let (_, resp) = sync_commit_unchecked(
        &f.client,
        &writer,
        &f.repo_id,
        &[("renamed.txt", b"v0", REG), ("other.txt", b"v0", REG)],
        &[],
        Some(&base),
    )
    .await;
    assert_eq!(
        resp.status(),
        403,
        "renaming a locked file must be rejected"
    );
}

/// The lock owner may delete the file, and the stale lock row must be swept in
/// the same request.
#[tokio::test]
async fn test_owner_delete_prunes_the_lock() {
    let f = TestFixture::new().await;

    let base = sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[("locked.txt", b"v0", REG), ("other.txt", b"v0", REG)],
        &[],
        None,
    )
    .await;
    lock_via_sync(&f, "/locked.txt", &f.sync_token).await;

    sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[("other.txt", b"v0", REG)],
        &[],
        Some(&base),
    )
    .await;

    assert_locked_files_empty(&f).await;
}

/// A lock on a path that does not exist is meaningless; the commit-time sweep
/// removes it so it cannot block a later file at the same path.
#[tokio::test]
async fn test_lock_on_a_missing_path_is_swept_after_a_commit() {
    let f = TestFixture::new().await;

    let base = sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[("other.txt", b"v0", REG)],
        &[],
        None,
    )
    .await;
    lock_via_sync(&f, "ghost.txt", &f.sync_token).await;

    sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[("other.txt", b"v1", REG)],
        &[],
        Some(&base),
    )
    .await;

    assert_locked_files_empty(&f).await;
}
