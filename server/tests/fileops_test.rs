mod common;

use common::TestFixture;

async fn upload_test_file(f: &TestFixture, name: &str, data: &[u8], parent_dir: &str) {
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, parent_dir, name, data)
        .await;
    assert_eq!(
        resp.status(),
        200,
        "upload {} failed: {:?}",
        name,
        resp.text().await
    );
}

async fn create_subdir(f: &TestFixture, path: &str) {
    let resp = f.client.create_dir(&f.api_token, &f.repo_id, path).await;
    // seahub answers 201 Created for mkdir.
    assert_eq!(
        resp.status(),
        201,
        "mkdir {} failed: {:?}",
        path,
        resp.text().await
    );
}

async fn dir_entry_count(f: &TestFixture, path: &str) -> usize {
    let resp = f.client.list_dir(&f.api_token, &f.repo_id, path).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    body.as_array().map(|a| a.len()).unwrap_or_else(|| {
        // v2.1 format wraps in object
        body.get("dirent_list")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0)
    })
}

#[tokio::test]
async fn test_batch_delete_single_file() {
    let f = TestFixture::new().await;

    upload_test_file(&f, "file1.txt", b"content1", "/").await;
    upload_test_file(&f, "file2.txt", b"content2", "/").await;

    assert!(
        dir_entry_count(&f, "/").await >= 2,
        "expected at least 2 entries in root"
    );

    let resp = f
        .client
        .batch_delete(&f.api_token, &f.repo_id, "/", &["file1.txt"])
        .await;
    assert_eq!(
        resp.status(),
        200,
        "batch delete failed: {:?}",
        resp.text().await
    );

    let resp = f.client.list_dir(&f.api_token, &f.repo_id, "/").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let names: Vec<&str> = body
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v["name"].as_str())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    assert!(!names.contains(&"file1.txt"), "file1 should be deleted");
    assert!(names.contains(&"file2.txt"), "file2 should remain");
}

#[tokio::test]
async fn test_batch_delete_multiple_files() {
    let f = TestFixture::new().await;

    upload_test_file(&f, "a.txt", b"aaa", "/").await;
    upload_test_file(&f, "b.txt", b"bbb", "/").await;
    upload_test_file(&f, "c.txt", b"ccc", "/").await;

    let resp = f
        .client
        .batch_delete(&f.api_token, &f.repo_id, "/", &["a.txt", "b.txt"])
        .await;
    assert_eq!(resp.status(), 200);

    let resp = f.client.list_dir(&f.api_token, &f.repo_id, "/").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let names: Vec<&str> = body
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v["name"].as_str())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    assert!(!names.contains(&"a.txt"));
    assert!(!names.contains(&"b.txt"));
    assert!(names.contains(&"c.txt"));
}

#[tokio::test]
async fn test_batch_delete_empty_list() {
    let f = TestFixture::new().await;

    upload_test_file(&f, "keep.txt", b"keep", "/").await;

    let resp = f
        .client
        .batch_delete(&f.api_token, &f.repo_id, "/", &[])
        .await;
    assert_eq!(resp.status(), 200);

    let resp = f.client.list_dir(&f.api_token, &f.repo_id, "/").await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_batch_delete_reloaddir() {
    let f = TestFixture::new().await;

    upload_test_file(&f, "del1.txt", b"del1", "/").await;
    upload_test_file(&f, "del2.txt", b"del2", "/").await;

    // seahub's reserved `_Internal` tree exists in every repo; the reloaddir listing must not
    // reveal it (or its contents) even though the deleter can read the rest of the directory.
    create_subdir(&f, "/_Internal").await;
    upload_test_file(&f, "index.db", b"internal", "/_Internal").await;

    let resp = f
        .client
        .batch_delete_with_dir(&f.api_token, &f.repo_id, "/", &["del1.txt", "del2.txt"])
        .await;
    assert_eq!(resp.status(), 200);
    // The same `oid`/`dir_perm` headers every other reloaddir response carries.
    assert!(
        resp.headers().contains_key("oid"),
        "reloaddir must report the parent's oid"
    );
    assert_eq!(
        resp.headers().get("dir_perm").map(|v| v.to_str().unwrap()),
        Some("rw")
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    // seahub's `reloaddir_if_necessary` returns the parent directory's dirents
    // as a bare array (the iOS client rejects any other wrapper).
    let arr = body
        .as_array()
        .unwrap_or_else(|| panic!("reloaddir=true must return a bare array, got {body:?}"));
    assert!(
        !arr.iter().any(|e| e["name"] == "del1.txt"),
        "deleted entry still listed: {body:?}"
    );
    assert!(
        !arr.iter().any(|e| e["name"] == "del2.txt"),
        "deleted entry still listed: {body:?}"
    );
    assert!(
        !arr.iter().any(|e| e["name"] == "_Internal"),
        "the reserved tree leaked into the listing: {body:?}"
    );
}

#[tokio::test]
async fn test_batch_delete_no_auth() {
    let f = TestFixture::new().await;

    upload_test_file(&f, "secret.txt", b"secret", "/").await;

    let resp = f
        .client
        .batch_delete("invalid-token", &f.repo_id, "/", &["secret.txt"])
        .await;
    assert!(
        resp.status() == 401 || resp.status() == 403,
        "expected 401 or 403, got {}",
        resp.status()
    );
}

#[tokio::test]
async fn test_batch_copy_same_repo() {
    let f = TestFixture::new().await;

    upload_test_file(&f, "src.txt", b"source content", "/").await;
    create_subdir(&f, "/dest").await;

    let resp = f
        .client
        .batch_copy(
            &f.api_token,
            &f.repo_id,
            "/",
            &["src.txt"],
            &f.repo_id,
            "/dest",
        )
        .await;
    assert_eq!(
        resp.status(),
        200,
        "batch copy failed: {:?}",
        resp.text().await
    );

    let body: serde_json::Value = resp.json().await.unwrap();
    let results = body.as_array().expect("response should be a JSON array");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["repo_id"].as_str(), Some(f.repo_id.as_str()));
    assert_eq!(results[0]["parent_dir"].as_str(), Some("/dest"));
    assert_eq!(results[0]["obj_name"].as_str(), Some("src.txt"));

    let resp = f.client.list_dir(&f.api_token, &f.repo_id, "/dest").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let names: Vec<&str> = body
        .as_array()
        .map(|a| a.iter().filter_map(|v| v["name"].as_str()).collect())
        .unwrap_or_default();
    assert!(
        names.contains(&"src.txt"),
        "copied file should exist in /dest"
    );
}

#[tokio::test]
async fn test_batch_copy_name_collision() {
    let f = TestFixture::new().await;

    upload_test_file(&f, "collide.txt", b"original", "/").await;
    create_subdir(&f, "/target").await;
    upload_test_file(&f, "collide.txt", b"original", "/target").await;

    let resp = f
        .client
        .batch_copy(
            &f.api_token,
            &f.repo_id,
            "/",
            &["collide.txt"],
            &f.repo_id,
            "/target",
        )
        .await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let obj_name = body[0]["obj_name"].as_str().unwrap();
    assert_ne!(obj_name, "collide.txt", "should auto-rename on collision");
    assert!(
        obj_name.contains("collide"),
        "name should contain original stem"
    );
}

#[tokio::test]
async fn test_batch_copy_cross_repo_fails() {
    let f = TestFixture::new().await;

    upload_test_file(&f, "xfer.txt", b"data", "/").await;

    let resp = f
        .client
        .batch_copy(
            &f.api_token,
            &f.repo_id,
            "/",
            &["xfer.txt"],
            "nonexistent-repo-id",
            "/",
        )
        .await;
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn test_batch_move_same_repo() {
    let f = TestFixture::new().await;

    upload_test_file(&f, "moveme.txt", b"move content", "/").await;
    create_subdir(&f, "/sub").await;

    let resp = f
        .client
        .batch_move(
            &f.api_token,
            &f.repo_id,
            "/",
            &["moveme.txt"],
            &f.repo_id,
            "/sub",
        )
        .await;
    assert_eq!(
        resp.status(),
        200,
        "batch move failed: {:?}",
        resp.text().await
    );

    let body: serde_json::Value = resp.json().await.unwrap();
    let results = body.as_array().expect("response should be a JSON array");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["repo_id"].as_str(), Some(f.repo_id.as_str()));
    assert_eq!(results[0]["parent_dir"].as_str(), Some("/sub"));
    assert_eq!(results[0]["obj_name"].as_str(), Some("moveme.txt"));

    let resp = f.client.list_dir(&f.api_token, &f.repo_id, "/sub").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let sub_names: Vec<&str> = body
        .as_array()
        .map(|a| a.iter().filter_map(|v| v["name"].as_str()).collect())
        .unwrap_or_default();
    assert!(sub_names.contains(&"moveme.txt"));

    let resp = f.client.list_dir(&f.api_token, &f.repo_id, "/").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let root_names: Vec<&str> = body
        .as_array()
        .map(|a| a.iter().filter_map(|v| v["name"].as_str()).collect())
        .unwrap_or_default();
    assert!(!root_names.contains(&"moveme.txt"));
}

#[tokio::test]
async fn test_batch_move_name_collision() {
    let f = TestFixture::new().await;

    upload_test_file(&f, "conflict.txt", b"source", "/").await;
    create_subdir(&f, "/dest").await;
    upload_test_file(&f, "conflict.txt", b"destination", "/dest").await;

    let resp = f
        .client
        .batch_move(
            &f.api_token,
            &f.repo_id,
            "/",
            &["conflict.txt"],
            &f.repo_id,
            "/dest",
        )
        .await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let obj_name = body[0]["obj_name"].as_str().unwrap();
    assert_ne!(obj_name, "conflict.txt", "should auto-rename on collision");
}

#[tokio::test]
async fn test_batch_move_cross_repo_fails() {
    let f = TestFixture::new().await;

    upload_test_file(&f, "mymove.txt", b"data", "/").await;

    let resp = f
        .client
        .batch_move(
            &f.api_token,
            &f.repo_id,
            "/",
            &["mymove.txt"],
            "bad-repo",
            "/",
        )
        .await;
    assert_eq!(resp.status(), 400);
}

// Self-subtree moves/copies must be refused: the tree update is a two-phase
// remove-then-add, so moving a directory into its own subtree used to commit the removal
// and then fail to resolve the destination (subtree gone, no trash entry, 500). A copy
// in the same shape creates a reference cycle because copies share fs_ids.

#[tokio::test]
async fn move_directory_into_its_own_subtree_is_rejected() {
    let f = TestFixture::new().await;

    create_subdir(&f, "/important").await;
    create_subdir(&f, "/important/sub").await;
    upload_test_file(&f, "keep.txt", b"payload", "/important").await;

    let resp = f
        .client
        .batch_move(
            &f.api_token,
            &f.repo_id,
            "/",
            &["important"],
            &f.repo_id,
            "/important/sub",
        )
        .await;
    assert_eq!(
        resp.status(),
        400,
        "moving a directory into its own subtree must be rejected, got: {}",
        resp.status()
    );

    // The source subtree is still intact — the removal must not have been
    // committed before the destination was validated.
    assert_eq!(dir_entry_count(&f, "/important").await, 2);
    assert_eq!(dir_entry_count(&f, "/important/sub").await, 0);

    let resp = f
        .client
        .batch_move(
            &f.api_token,
            &f.repo_id,
            "/",
            &["important"],
            &f.repo_id,
            "/important",
        )
        .await;
    assert_eq!(resp.status(), 400);
    assert_eq!(dir_entry_count(&f, "/important").await, 2);
}

#[tokio::test]
async fn copy_directory_into_its_own_subtree_is_rejected() {
    let f = TestFixture::new().await;

    create_subdir(&f, "/a").await;
    create_subdir(&f, "/a/b").await;

    let resp = f
        .client
        .batch_copy(&f.api_token, &f.repo_id, "/", &["a"], &f.repo_id, "/a/b")
        .await;
    assert_eq!(
        resp.status(),
        400,
        "copying a directory into its own subtree would create a cycle, got: {}",
        resp.status()
    );
    assert_eq!(dir_entry_count(&f, "/a/b").await, 0);

    create_subdir(&f, "/c").await;
    let resp = f
        .client
        .batch_copy(&f.api_token, &f.repo_id, "/", &["a"], &f.repo_id, "/c")
        .await;
    assert_eq!(resp.status(), 200, "a legitimate copy must still work");
    assert_eq!(dir_entry_count(&f, "/c").await, 1);
}

/// Sorted names of a directory's entries (handles the v2.1 array and the wrapped `dirent_list`).
async fn dir_names(f: &TestFixture, path: &str) -> Vec<String> {
    let resp = f.client.list_dir(&f.api_token, &f.repo_id, path).await;
    assert_eq!(resp.status(), 200, "list {path} failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = match body.as_array() {
        Some(a) => a.clone(),
        None => body
            .get("dirent_list")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default(),
    };
    let mut names: Vec<String> = arr
        .iter()
        .filter_map(|e| e["name"].as_str().map(str::to_string))
        .collect();
    names.sort();
    names
}

async fn dir_entries(f: &TestFixture, path: &str) -> Vec<(String, String)> {
    let resp = f.client.list_dir(&f.api_token, &f.repo_id, path).await;
    assert_eq!(resp.status(), 200, "list {path} failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = match body.as_array() {
        Some(a) => a.clone(),
        None => body
            .get("dirent_list")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default(),
    };
    let mut entries: Vec<(String, String)> = arr
        .iter()
        .filter_map(|e| {
            Some((
                e["name"].as_str()?.to_string(),
                e["type"].as_str().unwrap_or_default().to_string(),
            ))
        })
        .collect();
    entries.sort();
    entries
}

/// HEAD of the fixture's repo, to prove a failed move commits nothing.
async fn repo_head(f: &TestFixture) -> Option<String> {
    use sea_orm::EntityTrait;
    infra::entity::repo::Entity::find_by_id(f.repo_id.clone())
        .one(&*f.server.db)
        .await
        .unwrap()
        .expect("repo row")
        .head_commit_id
}

/// Both tree edits are stored before one commit moves HEAD, so a move that cannot
/// finish (here: the destination does not exist) must leave the source in place
/// and HEAD unchanged instead of committing the removal on its own.
#[tokio::test]
async fn a_move_to_a_missing_destination_leaves_the_source_untouched() {
    let f = TestFixture::new().await;
    create_subdir(&f, "/a").await;
    upload_test_file(&f, "f.txt", b"payload", "/a").await;
    let before = repo_head(&f).await;

    let resp = f
        .client
        .move_file(&f.api_token, &f.repo_id, "/a/f.txt", &f.repo_id, "/missing")
        .await;
    assert!(
        resp.status().is_client_error(),
        "move to a missing directory was accepted: {}",
        resp.status()
    );

    let resp = f
        .client
        .batch_move(
            &f.api_token,
            &f.repo_id,
            "/a",
            &["f.txt"],
            &f.repo_id,
            "/missing",
        )
        .await;
    assert!(
        resp.status().is_client_error(),
        "batch move to a missing directory was accepted: {}",
        resp.status()
    );

    assert_eq!(dir_names(&f, "/a").await, vec!["f.txt"]);
    assert_eq!(
        repo_head(&f).await,
        before,
        "a rejected move must not advance HEAD"
    );
}

/// A head commit that cannot be read is not an empty library. Treating the two
/// alike based the next directory commit on an empty tree, which replaced HEAD's
/// whole tree with the one new directory.
#[tokio::test]
async fn creating_a_directory_does_not_treat_a_missing_head_commit_as_empty() {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

    let f = TestFixture::new().await;
    create_subdir(&f, "/a").await;
    upload_test_file(&f, "f.txt", b"payload", "/a").await;
    let head = repo_head(&f).await.expect("head after upload");

    let commits_before = infra::entity::commit::Entity::find()
        .filter(infra::entity::commit::Column::RepoId.eq(f.repo_id.clone()))
        .all(&*f.server.db)
        .await
        .unwrap()
        .len();

    // Break the head pointer: the repo names a commit that no longer exists.
    infra::entity::commit::Entity::delete_many()
        .filter(infra::entity::commit::Column::CommitId.eq(head))
        .exec(&*f.server.db)
        .await
        .unwrap();

    let resp = f
        .client
        .create_dir(&f.api_token, &f.repo_id, "/still-here")
        .await;
    assert!(
        !resp.status().is_success(),
        "a broken head must not be reported as an empty repository"
    );

    let commits_after = infra::entity::commit::Entity::find()
        .filter(infra::entity::commit::Column::RepoId.eq(f.repo_id.clone()))
        .all(&*f.server.db)
        .await
        .unwrap()
        .len();
    assert_eq!(
        commits_after,
        commits_before - 1,
        "no replacement commit may be written on top of an empty tree"
    );
}

/// The commit id is a hash over the stored description. Hashing the requested
/// name while storing the de-duplicated one (`name (1).ext`) made the persisted
/// `commit_id` not match its own row and showed the wrong name in history.
#[tokio::test]
async fn a_kept_both_upload_stores_the_description_it_hashes() {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

    let f = TestFixture::new().await;
    for payload in [b"first".as_slice(), b"second".as_slice()] {
        let resp = f
            .client
            .upload_file_with_replace(&f.api_token, &f.repo_id, "/", "dup.txt", payload, false)
            .await;
        assert_eq!(resp.status(), 200, "keep-both upload failed");
    }

    let head = repo_head(&f).await.expect("head after upload");
    let row = infra::entity::commit::Entity::find()
        .filter(infra::entity::commit::Column::RepoId.eq(f.repo_id.clone()))
        .filter(infra::entity::commit::Column::CommitId.eq(head))
        .one(&*f.server.db)
        .await
        .unwrap()
        .expect("head commit row");

    assert!(
        row.description.contains("dup (1).txt"),
        "description names the stored file, got {:?}",
        row.description
    );

    let data = base::common::CommitData {
        commit_id: row.commit_id.clone(),
        repo_id: row.repo_id.clone(),
        root_id: row.root_id.clone(),
        creator_name: row.creator_name.clone(),
        creator: row.creator.clone(),
        description: row.description.clone(),
        ctime: row.ctime,
        parent_id: row.parent_id.clone(),
        second_parent_id: row.second_parent_id.clone(),
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
        version: i32::from(row.version),
        conflict: None,
        new_merge: None,
    };
    assert_eq!(
        server::domain::commit::compute_commit_id(&data),
        row.commit_id,
        "the stored description must reproduce the stored commit id"
    );
}

/// Moving a file into an ancestor of its own directory used to reuse the
/// destination object read before the removal. That object still pointed at the
/// subtree containing the file, so phase 2 copied the pre-removal subtree back
/// and the entry ended up at both paths.
#[tokio::test]
async fn moving_a_file_into_an_ancestor_keeps_one_copy() {
    let f = TestFixture::new().await;

    create_subdir(&f, "/a").await;
    create_subdir(&f, "/a/b").await;
    create_subdir(&f, "/a/b/c").await;
    upload_test_file(&f, "f.txt", b"payload", "/a/b/c").await;

    let resp = f
        .client
        .move_file(&f.api_token, &f.repo_id, "/a/b/c/f.txt", &f.repo_id, "/a")
        .await;
    assert_eq!(resp.status(), 200, "move failed: {:?}", resp.text().await);

    assert_eq!(dir_names(&f, "/a").await, vec!["b", "f.txt"]);
    assert_eq!(
        dir_names(&f, "/a/b/c").await,
        Vec::<String>::new(),
        "the old path must not keep a copy"
    );
    assert_eq!(dir_names(&f, "/a/b").await, vec!["c"]);

    let resp = f
        .client
        .download_file(&f.api_token, &f.repo_id, "/a/f.txt")
        .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.bytes().await.unwrap().to_vec(), b"payload");
}

/// The same bug for a directory: the moved subtree must not stay at its old path.
#[tokio::test]
async fn moving_a_directory_into_an_ancestor_keeps_its_subtree_once() {
    let f = TestFixture::new().await;

    create_subdir(&f, "/a").await;
    create_subdir(&f, "/a/b").await;
    create_subdir(&f, "/a/b/c").await;
    upload_test_file(&f, "f.txt", b"payload", "/a/b/c").await;

    let resp = f
        .client
        .move_file(&f.api_token, &f.repo_id, "/a/b/c", &f.repo_id, "/a")
        .await;
    assert_eq!(resp.status(), 200, "move failed: {:?}", resp.text().await);

    assert_eq!(dir_names(&f, "/a").await, vec!["b", "c"]);
    assert_eq!(
        dir_names(&f, "/a/b").await,
        Vec::<String>::new(),
        "the directory must be gone from its old parent"
    );
    assert_eq!(dir_names(&f, "/a/c").await, vec!["f.txt"]);
}

/// `rename_entry` used to find the entry by object id. `EMPTY_SHA1` is the id of every
/// empty directory *and* zero-byte file, so a directory holding both could rename the
/// wrong sibling; the assertion is on each entry's *type*, because the name set is the same.
#[tokio::test]
async fn renaming_a_zero_byte_file_next_to_an_empty_directory_renames_the_file() {
    let f = TestFixture::new().await;

    create_subdir(&f, "/d").await;
    create_subdir(&f, "/d/zzz").await;
    // A zero-byte file, created through the v2.1 endpoint (the upload route
    // stores no object for an empty file, its id is the `EMPTY_SHA1` sentinel).
    let resp = f
        .client
        .post_json(
            &format!("/api/v2.1/repos/{}/file/", f.repo_id),
            Some(&f.api_token),
            &serde_json::json!({"p": "/d/zero.txt"}),
        )
        .await;
    assert_eq!(resp.status(), 200, "create failed: {:?}", resp.text().await);
    assert_eq!(
        dir_entries(&f, "/d").await,
        vec![
            ("zero.txt".to_string(), "file".to_string()),
            ("zzz".to_string(), "dir".to_string()),
        ]
    );

    let resp = f
        .client
        .rename_file(&f.api_token, &f.repo_id, "/d/zero.txt", "renamed.txt")
        .await;
    assert_eq!(resp.status(), 200, "rename failed: {:?}", resp.text().await);

    assert_eq!(
        dir_entries(&f, "/d").await,
        vec![
            ("renamed.txt".to_string(), "file".to_string()),
            ("zzz".to_string(), "dir".to_string()),
        ],
        "the file must be renamed and the empty directory left alone"
    );
}

/// `EMPTY_SHA1` is the id of an empty directory *and* of a zero-byte file, so descending
/// a path must check each entry's mode: a path inside a zero-byte file does not exist.
#[tokio::test]
async fn a_path_inside_a_zero_byte_file_does_not_resolve() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .post_json(
            &format!("/api/v2.1/repos/{}/file/", f.repo_id),
            Some(&f.api_token),
            &serde_json::json!({"p": "/zero.txt"}),
        )
        .await;
    assert_eq!(resp.status(), 200, "create failed: {:?}", resp.text().await);

    let resp = f
        .client
        .create_dir(&f.api_token, &f.repo_id, "/zero.txt/sub")
        .await;
    assert_eq!(
        resp.status(),
        404,
        "a zero-byte file has no children: {:?}",
        resp.text().await
    );

    // Positive control: an empty directory still accepts children.
    create_subdir(&f, "/empty").await;
    create_subdir(&f, "/empty/sub").await;
    assert_eq!(dir_names(&f, "/empty").await, vec!["sub"]);
}

/// The reserved `_Internal` tree is hidden from the recursive listing just like the plain one —
/// including its children, which would otherwise show up with `_Internal` as their `parent_dir`.
#[tokio::test]
async fn test_dir_recursive_hides_reserved_internal() {
    let f = TestFixture::new().await;

    f.client
        .upload_file(&f.api_token, &f.repo_id, "/", "visible.txt", b"a")
        .await;
    f.client
        .create_dir(&f.api_token, &f.repo_id, "/_Internal")
        .await;
    f.client
        .upload_file(
            &f.api_token,
            &f.repo_id,
            "/_Internal",
            "bookkeeping.db",
            b"b",
        )
        .await;
    f.client
        .create_dir(&f.api_token, &f.repo_id, "/_Internal/nested")
        .await;

    let resp = f
        .client
        .list_dir_with_params(&f.api_token, &f.repo_id, "/", Some("1"), None)
        .await;
    assert_eq!(resp.status(), 200);
    let entries: Vec<serde_json::Value> = resp.json().await.unwrap();

    assert!(
        entries.iter().all(|e| e["name"] != "_Internal"),
        "the reserved tree leaked into the recursive listing: {entries:?}"
    );
    assert!(
        entries
            .iter()
            .all(|e| !e["parent_dir"].as_str().unwrap_or("").contains("_Internal")),
        "children of the reserved tree leaked into the recursive listing: {entries:?}"
    );
    assert!(
        entries.iter().any(|e| e["name"] == "visible.txt"),
        "the recursive listing dropped ordinary entries: {entries:?}"
    );
}
