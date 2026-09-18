//! Server-side three-way merge of a non-fast-forward sync upload.
//!
//! `PUT /seafhttp/repo/{id}/commit/HEAD?head=<C>` used to answer 409 when `C`'s
//! parent was no longer HEAD. Upstream does not reject such an upload: it merges
//! `C`'s tree with the current HEAD and records the result as a commit with two
//! parents (`fast_forward_or_merge()`, `seafile-server/server/http-server.c`),
//! which is what these tests pin down — including the conflict rename
//! (`SFConflict`), the `new_merge`/`conflict` flags in the commit JSON the
//! official clients read, and that activity logging reports only what the merge
//! itself introduced.

mod common;

use std::sync::atomic::{AtomicU64, Ordering};

use base::common::{DirEntryData, FsDirData, FsFileData, S_IFREG};
use common::{TestFixture, sync_commit};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};

const ZERO_SHA1: &str = "0000000000000000000000000000000000000000";
const REG: i32 = S_IFREG;

/// A fresh 40-hex commit id (the content never matters, only uniqueness).
fn random_hex_id() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    common::fs_object_id(format!("merge-test:{nanos}:{seq}").as_bytes())
}

/// The fs id `common::sync_put_file` stores `content` under, so a test can tell
/// which side of a conflict a directory entry came from.
fn file_fs_id(content: &[u8]) -> String {
    let fs = FsFileData {
        block_ids: vec![infra::crypto::fs_id::sha1_hex(content)],
        size: content.len() as i64,
        obj_type: 1,
        version: 1,
    };
    common::fs_object_id(serde_json::to_string(&fs).unwrap().as_bytes())
}

/// HEAD commit id of the fixture's library.
async fn head(f: &TestFixture) -> String {
    let resp = f.client.get_head_commit(&f.sync_token, &f.repo_id).await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    body["head_commit_id"].as_str().unwrap().to_string()
}

/// The commit JSON exactly as an official client reads it.
async fn commit_json(f: &TestFixture, commit_id: &str) -> Value {
    let resp = f
        .client
        .get_commit(&f.sync_token, &f.repo_id, commit_id)
        .await;
    assert_eq!(resp.status(), 200);
    resp.json().await.unwrap()
}

/// `(name, id, size)` of every entry of a directory, sorted by name.
async fn listing(f: &TestFixture, path: &str) -> Vec<(String, String, i64)> {
    let resp = f.client.list_dir(&f.api_token, &f.repo_id, path).await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let mut out: Vec<(String, String, i64)> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["name"].as_str().unwrap().to_string(),
                e["id"].as_str().unwrap().to_string(),
                e["size"].as_i64().unwrap(),
            )
        })
        .collect();
    out.sort();
    out
}

async fn names(f: &TestFixture, path: &str) -> Vec<String> {
    listing(f, path)
        .await
        .into_iter()
        .map(|(n, _, _)| n)
        .collect()
}

/// The `(id, size)` of one entry of a directory.
async fn entry(f: &TestFixture, dir: &str, name: &str) -> (String, i64) {
    let entries = listing(f, dir).await;
    let (_, id, size) = entries
        .iter()
        .find(|(n, _, _)| n == name)
        .unwrap_or_else(|| panic!("{name} not in {dir}: {entries:?}"));
    (id.clone(), *size)
}

/// `sync_commit` bound to an explicit token and library — the encrypted-library
/// test has the server generate the id, so it cannot use the fixture's pair.
async fn commit_in(
    f: &TestFixture,
    sync_token: &str,
    repo_id: &str,
    entries: &[(&str, &[u8], i32)],
    parent: Option<&str>,
) -> String {
    sync_commit(&f.client, sync_token, repo_id, entries, &[], parent).await
}

/// Pack FS objects the way `recv-fs` expects: `id[40] ‖ u32be(len) ‖ zlib(data)`.
fn pack_fs_objects(entries: &[(String, String)]) -> Vec<u8> {
    let mut body = Vec::new();
    for (id, json) in entries {
        let packed = infra::serialization::pack_fs::compress_fs_data(json.as_bytes()).unwrap();
        body.extend_from_slice(id.as_bytes());
        body.extend_from_slice(&(packed.len() as u32).to_be_bytes());
        body.extend_from_slice(&packed);
    }
    body
}

/// Two clients that synced the same base commit and then changed *different*
/// files: the stale upload must be merged, not rejected.
#[tokio::test]
async fn test_merge_disjoint_changes() {
    let f = TestFixture::new().await;

    let base = sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[("base.txt", b"base", REG)],
        &[],
        None,
    )
    .await;
    // Client A: add a.txt (fast-forward).
    let head_a = sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[("base.txt", b"base", REG), ("a.txt", b"aaaa", REG)],
        &[],
        Some(&base),
    )
    .await;
    assert_eq!(head(&f).await, head_a);

    // Client B: still based on `base`, add b.txt. Its commit's parent is not the
    // new HEAD, so the server merges it instead of rejecting the upload.
    let head_b = sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[("base.txt", b"base", REG), ("b.txt", b"bbbb", REG)],
        &[],
        Some(&base),
    )
    .await;

    let merged = head(&f).await;
    assert_ne!(merged, head_a, "HEAD must not be A's commit");
    assert_ne!(merged, head_b, "HEAD must not be B's commit");

    let commit = commit_json(&f, &merged).await;
    assert_eq!(commit["parent_id"].as_str().unwrap(), head_a);
    assert_eq!(commit["second_parent_id"].as_str().unwrap(), head_b);
    // Integers, not booleans: `json_integer_value(true)` reads back as 0.
    assert_eq!(commit["new_merge"].as_i64().unwrap(), 1);
    assert!(
        commit["conflict"].is_null(),
        "a disjoint merge has no conflict: {commit}"
    );
    assert_eq!(
        commit["description"].as_str().unwrap(),
        "Auto merge by system"
    );
    assert_eq!(commit["creator_name"].as_str().unwrap(), "test@example.com");
    assert_eq!(commit["creator"].as_str().unwrap(), ZERO_SHA1);

    // The merged tree contains both sides' additions.
    assert_eq!(names(&f, "/").await, vec!["a.txt", "b.txt", "base.txt"]);

    // Activity logging must describe what the *merge* introduced. Diffing
    // against B's own base would report A's a.txt a second time (it was already
    // logged when A's commit landed). Creates are aggregated into one row per
    // (op_type, obj_type) within a 5-minute window, so the assertion is on the
    // recorded paths: every path appears exactly once.
    let rows = infra::entity::activity::Entity::find()
        .filter(infra::entity::activity::Column::RepoId.eq(&f.repo_id))
        .all(f.server.db.as_ref())
        .await
        .unwrap();
    let mut paths: Vec<String> = Vec::new();
    for row in &rows {
        let detail: Value = serde_json::from_str(&row.detail).unwrap_or(Value::Null);
        match detail {
            Value::Array(items) => paths.extend(
                items
                    .iter()
                    .filter_map(|i| i["path"].as_str().map(String::from)),
            ),
            Value::Object(_) => {
                if let Some(path) = detail["path"].as_str() {
                    paths.push(path.to_string());
                }
            }
            _ => {}
        }
    }
    paths.sort();
    // Only the files: an unrelated `/` entry (the library itself) is recorded
    // when the repository is created.
    let files: Vec<&str> = paths
        .iter()
        .filter(|p| p.ends_with(".txt"))
        .map(String::as_str)
        .collect();
    assert_eq!(
        files,
        vec!["/a.txt", "/b.txt", "/base.txt"],
        "each path is logged exactly once, by the commit that introduced it"
    );
}

/// Both clients changed the same file: the merge keeps HEAD's version under the
/// original name and writes the stale client's version to an `SFConflict` name.
#[tokio::test]
async fn test_merge_conflict_renames_stale_version() {
    let f = TestFixture::new().await;

    let base = sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[("f.txt", b"v0", REG)],
        &[],
        None,
    )
    .await;
    let head_a = sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[("f.txt", b"winner", REG)],
        &[],
        Some(&base),
    )
    .await;
    let head_b = sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[("f.txt", b"stale-version", REG)],
        &[],
        Some(&base),
    )
    .await;

    let merged = head(&f).await;
    let commit = commit_json(&f, &merged).await;
    assert_eq!(commit["conflict"].as_i64().unwrap(), 1);
    assert_eq!(commit["new_merge"].as_i64().unwrap(), 1);
    assert_eq!(commit["parent_id"].as_str().unwrap(), head_a);
    assert_eq!(commit["second_parent_id"].as_str().unwrap(), head_b);

    let entries = listing(&f, "/").await;
    assert_eq!(entries.len(), 2, "both versions must survive: {entries:?}");

    // HEAD's version keeps the original name...
    let (winner_id, winner_size) = entry(&f, "/", "f.txt").await;
    assert_eq!(winner_id, file_fs_id(b"winner"));
    assert_eq!(winner_size, b"winner".len() as i64);

    // ... and the stale one is renamed after the last modifier of the *remote*
    // tree's copy. `create_test_user` stores no display name, so the nickname
    // seahub would use is the email's local part.
    let conflict_name = entries
        .iter()
        .map(|(n, _, _)| n.clone())
        .find(|n| n != "f.txt")
        .expect("a renamed conflict entry");
    let prefix = "f (SFConflict test ";
    assert!(
        conflict_name.starts_with(prefix) && conflict_name.ends_with(").txt"),
        "unexpected conflict name: {conflict_name}"
    );
    let stamp = &conflict_name[prefix.len()..conflict_name.len() - ").txt".len()];
    assert_eq!(
        stamp.len(),
        "YYYY-MM-DD-HH-MM-SS".len(),
        "conflict names carry a local timestamp: {conflict_name}"
    );
    let (conflict_id, conflict_size) = entry(&f, "/", &conflict_name).await;
    assert_eq!(conflict_id, file_fs_id(b"stale-version"));
    assert_eq!(conflict_size, b"stale-version".len() as i64);
}

/// Delete on one side, modify on the other: the modified version survives — in
/// both directions, exactly like upstream's `threeway_merge()`.
#[tokio::test]
async fn test_merge_delete_versus_modify() {
    let f = TestFixture::new().await;

    // Half 1: A deletes f.txt, B (stale) modifies it.
    let base = sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[("f.txt", b"v0", REG), ("keep.txt", b"keep", REG)],
        &[],
        None,
    )
    .await;
    let head_a = sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[("keep.txt", b"keep", REG)],
        &[],
        Some(&base),
    )
    .await;
    sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[("f.txt", b"changed", REG), ("keep.txt", b"keep", REG)],
        &[],
        Some(&base),
    )
    .await;

    assert_eq!(names(&f, "/").await, vec!["f.txt", "keep.txt"]);
    assert_ne!(
        head(&f).await,
        head_a,
        "B's stale upload must have been merged, not rejected"
    );
    let (id, _) = entry(&f, "/", "f.txt").await;
    assert_eq!(id, file_fs_id(b"changed"), "the changed side wins");
    assert!(
        commit_json(&f, &head(&f).await).await["conflict"].is_null(),
        "keeping the changed side is not a conflict"
    );

    // Half 2, the mirror: A changes g.txt, B (stale) deletes it.
    let head_m1 = head(&f).await;
    let base2 = sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[
            ("f.txt", b"changed", REG),
            ("keep.txt", b"keep", REG),
            ("g.txt", b"v0", REG),
        ],
        &[],
        Some(&head_m1),
    )
    .await;
    let head_c = sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[
            ("f.txt", b"changed", REG),
            ("keep.txt", b"keep", REG),
            ("g.txt", b"head-version", REG),
        ],
        &[],
        Some(&base2),
    )
    .await;
    let head_b2 = sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[("f.txt", b"changed", REG), ("keep.txt", b"keep", REG)],
        &[],
        Some(&base2),
    )
    .await;

    assert!(
        !names(&f, "/")
            .await
            .iter()
            .any(|n| n.contains("SFConflict")),
        "head's version won, so nothing was renamed"
    );
    let (g_id, _) = entry(&f, "/", "g.txt").await;
    assert_eq!(g_id, file_fs_id(b"head-version"));
    let merged = head(&f).await;
    assert_ne!(merged, head_c);
    assert_ne!(merged, head_b2);
    let commit = commit_json(&f, &merged).await;
    assert_eq!(commit["parent_id"].as_str().unwrap(), head_c);
    assert_eq!(commit["second_parent_id"].as_str().unwrap(), head_b2);
}

/// A file on one side versus a directory on the other (D/F conflict): the
/// directory keeps the name and the file is renamed.
#[tokio::test]
async fn test_merge_dir_file_conflict() {
    let f = TestFixture::new().await;

    let base = sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[("x", b"v0", REG)],
        &[],
        None,
    )
    .await;
    // A replaces the file `x` with a directory `x`.
    let head_a = sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[],
        &[("x", &[("inner.txt", b"inner".as_slice(), REG)])],
        Some(&base),
    )
    .await;
    // B (stale) modifies the file `x`.
    sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[("x", b"stale", REG)],
        &[],
        Some(&base),
    )
    .await;

    let commit = commit_json(&f, &head(&f).await).await;
    assert_eq!(commit["conflict"].as_i64().unwrap(), 1);
    assert_eq!(commit["parent_id"].as_str().unwrap(), head_a);

    let entries = listing(&f, "/").await;
    assert_eq!(entries.len(), 2, "the directory and the file: {entries:?}");
    // The directory keeps the name; the file is renamed.
    assert_eq!(names(&f, "/x").await, vec!["inner.txt"]);
    let file = entries
        .iter()
        .find(|(n, _, _)| n != "x")
        .expect("the renamed file");
    assert!(file.0.starts_with("x (SFConflict test "), "{file:?}");
    assert_eq!(file.1, file_fs_id(b"stale"));
}

/// Concurrent changes inside the same sub-directory merge recursively.
#[tokio::test]
async fn test_merge_nested_directory() {
    let f = TestFixture::new().await;

    let base = sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[],
        &[("d", &[("f1", b"1".as_slice(), REG)])],
        None,
    )
    .await;
    sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[],
        &[(
            "d",
            &[("f1", b"1".as_slice(), REG), ("f2", b"2".as_slice(), REG)],
        )],
        Some(&base),
    )
    .await;
    sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[],
        &[(
            "d",
            &[("f1", b"1".as_slice(), REG), ("f3", b"3".as_slice(), REG)],
        )],
        Some(&base),
    )
    .await;

    let commit = commit_json(&f, &head(&f).await).await;
    assert_eq!(commit["new_merge"].as_i64().unwrap(), 1);
    assert!(
        commit["conflict"].is_null(),
        "disjoint additions are not a conflict"
    );
    assert_eq!(names(&f, "/d").await, vec!["f1", "f2", "f3"]);
}

/// A commit whose parent is the `EMPTY_SHA1` sentinel while the library does
/// have a HEAD (a second client that cloned the library while it was empty) is
/// merged against the empty tree.
#[tokio::test]
async fn test_merge_with_empty_sentinel_parent() {
    let f = TestFixture::new().await;

    let head_a = sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[("a.txt", b"aaaa", REG)],
        &[],
        None,
    )
    .await;
    let head_b = sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[("b.txt", b"bbbb", REG)],
        &[],
        Some(ZERO_SHA1),
    )
    .await;

    let merged = head(&f).await;
    assert_ne!(merged, head_b);
    let commit = commit_json(&f, &merged).await;
    assert_eq!(commit["parent_id"].as_str().unwrap(), head_a);
    assert_eq!(commit["second_parent_id"].as_str().unwrap(), head_b);
    assert_eq!(commit["new_merge"].as_i64().unwrap(), 1);
    assert_eq!(names(&f, "/").await, vec!["a.txt", "b.txt"]);
}

/// Replaying the same `head` after it became HEAD is a no-op, not a second
/// merge (upstream's fast-forward branch).
#[tokio::test]
async fn test_update_branch_is_idempotent_for_unchanged_head() {
    let f = TestFixture::new().await;

    let commit = sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[("a.txt", b"aaaa", REG)],
        &[],
        None,
    )
    .await;
    let resp = f
        .client
        .update_branch(&f.sync_token, &f.repo_id, &commit)
        .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(head(&f).await, commit, "no merge commit for a replay");

    let rows = infra::entity::commit::Entity::find()
        .filter(infra::entity::commit::Column::RepoId.eq(&f.repo_id))
        .all(f.server.db.as_ref())
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "a replay must not add a commit");
}

/// A block the upload references but never sent is reported as missing (446)
/// *before* any merge, and HEAD must not move.
#[tokio::test]
async fn test_merge_missing_block_is_446() {
    let f = TestFixture::new().await;

    let base = sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[("base.txt", b"base", REG)],
        &[],
        None,
    )
    .await;
    let head_a = sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[("base.txt", b"base", REG), ("a.txt", b"aaaa", REG)],
        &[],
        Some(&base),
    )
    .await;

    // B is stale (parent = base) *and* references a block that was never
    // uploaded.
    let now = chrono::Utc::now().timestamp();
    let file = FsFileData {
        block_ids: vec!["a".repeat(40)],
        size: 4,
        obj_type: 1,
        version: 1,
    };
    let file_json = serde_json::to_string(&file).unwrap();
    let file_id = common::fs_object_id(file_json.as_bytes());
    let root = FsDirData {
        dirents: vec![DirEntryData {
            id: file_id.clone(),
            mode: REG,
            modifier: "test@example.com".to_string(),
            mtime: now,
            name: "c.txt".to_string(),
            size: 4,
        }],
        obj_type: 3,
        version: 1,
    };
    let root_json = serde_json::to_string(&root).unwrap();
    let root_id = common::fs_object_id(root_json.as_bytes());
    let resp = f
        .client
        .recv_fs(
            &f.sync_token,
            &f.repo_id,
            pack_fs_objects(&[(file_id, file_json), (root_id.clone(), root_json)]),
        )
        .await;
    assert_eq!(resp.status(), 200);

    let commit_id = random_hex_id();
    let body = serde_json::to_string(&json!({
        "commit_id": commit_id,
        "repo_id": f.repo_id,
        "root_id": root_id,
        "creator_name": "test@example.com",
        "creator": ZERO_SHA1,
        "description": "stale commit with a missing block",
        "ctime": now,
        "parent_id": base,
        "second_parent_id": null,
        "version": 1,
    }))
    .unwrap()
    .into_bytes();
    let resp = f
        .client
        .put_commit(&f.sync_token, &f.repo_id, &commit_id, body)
        .await;
    assert_eq!(resp.status(), 200);

    let resp = f
        .client
        .update_branch(&f.sync_token, &f.repo_id, &commit_id)
        .await;
    assert_eq!(resp.status(), 446, "a missing block must not be merged");
    assert_eq!(head(&f).await, head_a, "HEAD must not move");
}

/// A merge inside an encrypted library produces a merge commit that still
/// carries the complete encryption block (`RepoCrypto` re-synthesises it from
/// the repo row on read), and the conflict flags survive that path too.
#[tokio::test]
async fn test_merge_in_encrypted_library() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .create_encrypted_repo_with_password(&f.api_token, "enc-merge", "secret-pass")
        .await;
    assert_eq!(resp.status(), 201, "encrypted library creation failed");
    let body: Value = resp.json().await.unwrap();
    let repo_id = body["id"].as_str().unwrap().to_string();
    let sync_token = common::get_sync_token(&f.client, &f.api_token, &repo_id).await;

    let base = commit_in(&f, &sync_token, &repo_id, &[("f.txt", b"v0", REG)], None).await;
    let head_a = commit_in(
        &f,
        &sync_token,
        &repo_id,
        &[("f.txt", b"winner", REG)],
        Some(&base),
    )
    .await;
    let head_b = commit_in(
        &f,
        &sync_token,
        &repo_id,
        &[("f.txt", b"stale-version", REG)],
        Some(&base),
    )
    .await;

    let resp = f.client.get_head_commit(&sync_token, &repo_id).await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let merged = body["head_commit_id"].as_str().unwrap().to_string();
    assert_ne!(merged, head_b);

    let resp = f.client.get_commit(&sync_token, &repo_id, &merged).await;
    assert_eq!(resp.status(), 200);
    let commit: Value = resp.json().await.unwrap();
    assert_eq!(commit["encrypted"].as_str().unwrap(), "true");
    assert_eq!(commit["enc_version"].as_i64().unwrap(), 2);
    assert!(!commit["magic"].as_str().unwrap_or_default().is_empty());
    assert!(!commit["key"].as_str().unwrap_or_default().is_empty());
    assert_eq!(commit["parent_id"].as_str().unwrap(), head_a);
    assert_eq!(commit["second_parent_id"].as_str().unwrap(), head_b);
    assert_eq!(commit["new_merge"].as_i64().unwrap(), 1);
    assert_eq!(commit["conflict"].as_i64().unwrap(), 1);

    let resp = f.client.list_dir(&f.api_token, &repo_id, "/").await;
    assert_eq!(resp.status(), 200);
    let entries: Value = resp.json().await.unwrap();
    let names: Vec<&str> = entries
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), 2, "both versions: {names:?}");
    assert!(names.iter().any(|n| n.starts_with("f (SFConflict ")));
}

/// The directory objects the merge writes are serialized in seafile's order
/// (descending by name), which is what makes their `fs_id` reproducible: the id
/// is the SHA-1 of that byte string.
#[tokio::test]
async fn test_merge_output_dirents_are_sorted_descending() {
    let f = TestFixture::new().await;

    let base = sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[("m.txt", b"m", REG)],
        &[],
        None,
    )
    .await;
    sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[("m.txt", b"m", REG), ("a.txt", b"a", REG)],
        &[],
        Some(&base),
    )
    .await;
    sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[("m.txt", b"m", REG), ("z.txt", b"z", REG)],
        &[],
        Some(&base),
    )
    .await;

    let commit = commit_json(&f, &head(&f).await).await;
    let root_id = commit["root_id"].as_str().unwrap().to_string();
    let model = infra::entity::fs_object::Entity::find()
        .filter(infra::entity::fs_object::Column::RepoId.eq(&f.repo_id))
        .filter(infra::entity::fs_object::Column::FsId.eq(&root_id))
        .one(f.server.db.as_ref())
        .await
        .unwrap()
        .expect("the merged root object must be stored");
    let dirents: Value = serde_json::from_str::<Value>(&model.data).unwrap()["dirents"].clone();
    let order: Vec<&str> = dirents
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        order,
        vec!["z.txt", "m.txt", "a.txt"],
        "descending `strcmp` order, like `compare_dirents()`"
    );
    // The id has to be the hash of exactly those bytes.
    assert_eq!(
        common::fs_object_id(model.data.as_bytes()),
        root_id,
        "the merged object id must hash its serialized form"
    );
}
