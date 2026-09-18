//! Repository size accounting on the sync commit path.
//!
//! A library's `repo.size` — and therefore a user's usage, which is
//! `SUM(repos.size)` over the libraries they own — is maintained incrementally
//! from the size delta of each commit. That delta used to count additions and
//! modifications only: `check_commit_blocks` walked the *new* tree, so a commit
//! that deleted a file or a whole directory subtracted nothing and left the
//! library permanently inflated. Client deletions are the normal way a desktop
//! or mobile client removes data, so quota freed that way was never reclaimed.
//!
//! These tests pin the delta to `size(new) - size(base)` across the sync surface:
//! file and directory deletion, emptying a library (both as a stored
//! empty-directory object and as the `EMPTY_SHA1` sentinel), renames,
//! file/directory swaps, unchanged subtrees next to a change, the server-side
//! merge path, and the user-visible consequence that deleting through a client
//! frees quota again.

mod common;

use std::sync::atomic::{AtomicU64, Ordering};

use base::common::S_IFREG;
use common::{SyncFileSpec, TestFixture, fs_object_id, sync_commit};
use sea_orm::EntityTrait;
use serde_json::Value;

const EMPTY_SHA1: &str = "0000000000000000000000000000000000000000";
const REG: i32 = S_IFREG;

/// A fresh 40-hex commit id (only uniqueness matters).
fn random_hex_id() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    fs_object_id(format!("repo-size-test:{nanos}:{seq}").as_bytes())
}

/// The `size` column the incremental accounting maintains.
async fn repo_size(f: &TestFixture) -> i64 {
    infra::entity::repo::Entity::find_by_id(f.repo_id.clone())
        .one(&*f.server.db)
        .await
        .unwrap()
        .expect("repo row")
        .size
}

/// The usage a client is shown (`GET /api2/account/info/`), i.e. what quota is
/// enforced against.
async fn account_usage(f: &TestFixture) -> i64 {
    let resp = f
        .client
        .get("/api2/account/info/", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    body["usage"].as_i64().unwrap()
}

async fn head(f: &TestFixture) -> String {
    let resp = f.client.get_head_commit(&f.sync_token, &f.repo_id).await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    body["head_commit_id"].as_str().unwrap().to_string()
}

async fn names(f: &TestFixture, path: &str) -> Vec<String> {
    let resp = f.client.list_dir(&f.api_token, &f.repo_id, path).await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let mut out: Vec<String> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap().to_string())
        .collect();
    out.sort();
    out
}

/// `sync_commit` bound to the fixture's library.
async fn commit(
    f: &TestFixture,
    entries: &[SyncFileSpec<'_>],
    dirs: &[(&str, &[SyncFileSpec<'_>])],
    parent: Option<&str>,
) -> String {
    sync_commit(&f.client, &f.sync_token, &f.repo_id, entries, dirs, parent).await
}

/// Put a commit pointing at an arbitrary root fs id, then advance HEAD to it.
///
/// `sync_commit` always stores a directory object (even for an empty tree), but
/// an official client represents "no entries" as the `EMPTY_SHA1` sentinel, which
/// has no object at all — that case needs its own path.
async fn put_root_commit(f: &TestFixture, root_id: &str, parent: Option<&str>) -> String {
    let commit_id = random_hex_id();
    let commit = base::common::CommitData {
        commit_id: commit_id.clone(),
        repo_id: f.repo_id.clone(),
        root_id: root_id.to_string(),
        creator_name: f.email.clone(),
        creator: EMPTY_SHA1.to_string(),
        description: "sync commit".to_string(),
        ctime: chrono::Utc::now().timestamp(),
        parent_id: parent.map(|p| p.to_string()),
        second_parent_id: None,
        repo_name: None,
        repo_desc: None,
        repo_category: None,
        encrypted: None,
        enc_version: None,
        magic: None,
        salt: None,
        pwd_hash: None,
        pwd_hash_algo: None,
        pwd_hash_params: None,
        key: None,
        version: 1,
        conflict: None,
        new_merge: None,
    };
    let body = serde_json::to_string(&commit).unwrap().into_bytes();
    let resp = f
        .client
        .put_commit(&f.sync_token, &f.repo_id, &commit_id, body)
        .await;
    assert_eq!(resp.status(), 200, "put_commit failed");
    let resp = f
        .client
        .update_branch(&f.sync_token, &f.repo_id, &commit_id)
        .await;
    assert_eq!(resp.status(), 200, "update_branch failed");
    commit_id
}

async fn set_user_quota(f: &TestFixture, quota: Option<i64>) {
    use sea_orm::{ActiveModelTrait, Set};

    let user_record = infra::entity::user::Entity::find_by_id(f.user_id)
        .one(&*f.server.db)
        .await
        .unwrap()
        .unwrap();
    let mut active: infra::entity::user::ActiveModel = user_record.into();
    active.storage_quota = Set(quota);
    active.update(&*f.server.db).await.unwrap();
}

#[tokio::test]
async fn sync_file_edit_adjusts_size_by_the_difference() {
    let f = TestFixture::new().await;

    let c1 = commit(&f, &[("a.txt", b"0123456789", REG)], &[], None).await;
    assert_eq!(repo_size(&f).await, 10);

    let c2 = commit(&f, &[("a.txt", b"0123456789abcde", REG)], &[], Some(&c1)).await;
    assert_eq!(repo_size(&f).await, 15);
    assert_eq!(account_usage(&f).await, 15);
    assert_eq!(head(&f).await, c2);
}

#[tokio::test]
async fn sync_file_delete_subtracts_its_size() {
    let f = TestFixture::new().await;

    let c1 = commit(
        &f,
        &[("a.txt", b"0123456789", REG), ("b.txt", b"01234", REG)],
        &[],
        None,
    )
    .await;
    assert_eq!(repo_size(&f).await, 15);

    // The new tree no longer lists a.txt.
    let _c2 = commit(&f, &[("b.txt", b"01234", REG)], &[], Some(&c1)).await;
    assert_eq!(repo_size(&f).await, 5, "a deletion must subtract its bytes");
    assert_eq!(account_usage(&f).await, 5);
}

#[tokio::test]
async fn sync_directory_delete_subtracts_the_whole_subtree() {
    let f = TestFixture::new().await;

    let twenty = vec![b'x'; 20];
    let d: Vec<SyncFileSpec<'_>> = vec![
        ("one.txt", b"0123456789", REG),
        ("two.txt", twenty.as_slice(), REG),
    ];
    let c1 = commit(&f, &[("top.txt", b"01234", REG)], &[("d", &d)], None).await;
    assert_eq!(repo_size(&f).await, 35);

    let _c2 = commit(&f, &[("top.txt", b"01234", REG)], &[], Some(&c1)).await;
    assert_eq!(
        repo_size(&f).await,
        5,
        "deleting a directory must drop its whole subtree, not just the entry"
    );
}

#[tokio::test]
async fn sync_emptied_root_object_subtracts_everything() {
    let f = TestFixture::new().await;

    let inner: Vec<SyncFileSpec<'_>> = vec![("in.txt", b"0123456789", REG)];
    let c1 = commit(&f, &[("a.txt", b"01234", REG)], &[("d", &inner)], None).await;
    assert_eq!(repo_size(&f).await, 15);

    // An empty root still names a stored directory object (our helper always
    // saves one), so the deletion is found by diffing the two roots.
    let _c2 = commit(&f, &[], &[], Some(&c1)).await;
    assert_eq!(repo_size(&f).await, 0);
    assert_eq!(account_usage(&f).await, 0);
    assert!(names(&f, "/").await.is_empty());
}

#[tokio::test]
async fn sync_empty_root_sentinel_subtracts_everything() {
    let f = TestFixture::new().await;

    let inner: Vec<SyncFileSpec<'_>> = vec![("in.txt", b"0123456789", REG)];
    let c1 = commit(&f, &[("a.txt", b"01234", REG)], &[("d", &inner)], None).await;
    assert_eq!(repo_size(&f).await, 15);

    // `EMPTY_SHA1` is how an official client says "the root has no entries"; it
    // is not a stored object, so it used to short-circuit the whole check.
    let _c2 = put_root_commit(&f, EMPTY_SHA1, Some(&c1)).await;
    assert_eq!(repo_size(&f).await, 0);
    assert_eq!(account_usage(&f).await, 0);
}

#[tokio::test]
async fn sync_rename_does_not_double_count() {
    let f = TestFixture::new().await;

    let c1 = commit(&f, &[("old.txt", b"0123456789", REG)], &[], None).await;
    assert_eq!(repo_size(&f).await, 10);

    // Same content, same fs id, new name: the diff sees one deletion and one
    // addition that cancel out.
    let _c2 = commit(&f, &[("new.txt", b"0123456789", REG)], &[], Some(&c1)).await;
    assert_eq!(repo_size(&f).await, 10);
    assert_eq!(names(&f, "/").await, vec!["new.txt"]);
}

#[tokio::test]
async fn sync_file_replaced_by_directory_accounts_for_both_sides() {
    let f = TestFixture::new().await;

    let c1 = commit(&f, &[("x", b"0123456789", REG)], &[], None).await;
    assert_eq!(repo_size(&f).await, 10);

    let thirty = vec![b'y'; 30];
    let forty = vec![b'z'; 40];
    let x: Vec<SyncFileSpec<'_>> = vec![
        ("p.txt", thirty.as_slice(), REG),
        ("q.txt", forty.as_slice(), REG),
    ];
    let _c2 = commit(&f, &[], &[("x", &x)], Some(&c1)).await;
    assert_eq!(
        repo_size(&f).await,
        70,
        "replacing a file with a directory must drop the file and add the subtree"
    );
}

#[tokio::test]
async fn sync_directory_replaced_by_file_subtracts_the_subtree() {
    let f = TestFixture::new().await;

    let thirty = vec![b'y'; 30];
    let forty = vec![b'z'; 40];
    let x: Vec<SyncFileSpec<'_>> = vec![
        ("p.txt", thirty.as_slice(), REG),
        ("q.txt", forty.as_slice(), REG),
    ];
    let c1 = commit(&f, &[], &[("x", &x)], None).await;
    assert_eq!(repo_size(&f).await, 70);

    // A directory entry carries no size of its own, so the whole base subtree
    // has to be walked to subtract it.
    let _c2 = commit(&f, &[("x", b"0123456789", REG)], &[], Some(&c1)).await;
    assert_eq!(repo_size(&f).await, 10);
}

#[tokio::test]
async fn sync_unchanged_subtree_is_skipped_without_drift() {
    let f = TestFixture::new().await;

    let twenty = vec![b'x'; 20];
    let d: Vec<SyncFileSpec<'_>> = vec![("in.txt", twenty.as_slice(), REG)];
    let c1 = commit(&f, &[("top.txt", b"01234", REG)], &[("d", &d)], None).await;
    assert_eq!(repo_size(&f).await, 25);

    // `d/` keeps the same fs id: the walk must skip it and still account for the
    // edit one level up.
    let c2 = commit(&f, &[("top.txt", b"012345", REG)], &[("d", &d)], Some(&c1)).await;
    assert_eq!(repo_size(&f).await, 26);
    assert_eq!(head(&f).await, c2);
}

/// The merge path measures its delta against the *head* tree (the stored size
/// already includes it), so a file the merged result no longer has must be
/// subtracted there too.
#[tokio::test]
async fn sync_merge_that_deletes_a_head_file_subtracts_it() {
    let f = TestFixture::new().await;

    let twenty = vec![b'b'; 20];
    let a = commit(
        &f,
        &[
            ("a.txt", b"0123456789", REG),
            ("b.txt", twenty.as_slice(), REG),
        ],
        &[],
        None,
    )
    .await;
    assert_eq!(repo_size(&f).await, 30);

    // HEAD moves on and gains a file.
    let _b = commit(
        &f,
        &[
            ("a.txt", b"0123456789", REG),
            ("b.txt", twenty.as_slice(), REG),
            ("h.txt", b"01234", REG),
        ],
        &[],
        Some(&a),
    )
    .await;
    assert_eq!(repo_size(&f).await, 35);

    // A second client, still based on `a`, deletes a.txt. HEAD no longer matches
    // its parent, so the server merges instead of fast-forwarding.
    let _merge = commit(&f, &[("b.txt", twenty.as_slice(), REG)], &[], Some(&a)).await;
    assert_eq!(
        names(&f, "/").await,
        vec!["b.txt", "h.txt"],
        "the merge keeps the head-only file and honours the client's deletion"
    );
    assert_eq!(
        repo_size(&f).await,
        25,
        "the deleted file's bytes were counted in the head size and must go"
    );
    assert_eq!(account_usage(&f).await, 25);
}

/// Restoring a historical revision repoints a dirent at another fs id, so the
/// library's size changes by the difference between the two versions — but the
/// restore commits without an `adjust_repo_size`, which used to leave the size
/// at the newer (or older) value.
#[tokio::test]
async fn file_revision_restore_adjusts_size_by_the_difference() {
    let f = TestFixture::new().await;

    let c1 = commit(&f, &[("a.txt", b"0123456789", REG)], &[], None).await;
    let _c2 = commit(&f, &[("a.txt", b"0123456789abcde", REG)], &[], Some(&c1)).await;
    assert_eq!(repo_size(&f).await, 15);

    let resp = f
        .client
        .post_json(
            &format!(
                "/api/v2.1/repos/{}/file/revision/restore/?p=/a.txt&commit_id={}",
                f.repo_id, c1
            ),
            Some(&f.api_token),
            &serde_json::json!({}),
        )
        .await;
    assert_eq!(resp.status(), 200, "restore revision failed");

    assert_eq!(
        repo_size(&f).await,
        10,
        "the restored version's bytes replace the ones the library held"
    );
    assert_eq!(account_usage(&f).await, 10);
}

/// Deleting through a client has to free quota, not just change a number in a
/// listing: quota is enforced against `SUM(repos.size)`.
#[tokio::test]
async fn deleting_through_the_client_frees_quota() {
    let f = TestFixture::new().await;
    set_user_quota(&f, Some(60)).await;

    let fifty = vec![b'a'; 50];
    let c1 = commit(&f, &[("a.txt", fifty.as_slice(), REG)], &[], None).await;
    assert_eq!(repo_size(&f).await, 50);
    assert_eq!(account_usage(&f).await, 50);

    // 50 + 30 > 60: the upload does not fit yet.
    let thirty = vec![b'b'; 30];
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "extra.txt", &thirty)
        .await;
    assert_eq!(resp.status(), 443, "usage must count against the quota");

    // Delete the file the way a desktop client does.
    let _c2 = commit(&f, &[], &[], Some(&c1)).await;
    assert_eq!(repo_size(&f).await, 0);
    assert_eq!(account_usage(&f).await, 0);

    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "fits.txt", &thirty)
        .await;
    assert_eq!(
        resp.status(),
        200,
        "bytes freed by a client deletion must be reusable"
    );
}
