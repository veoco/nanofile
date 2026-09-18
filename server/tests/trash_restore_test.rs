//! Restoring files and directories from a library's file trash.
//!
//! The delete paths subtract what they take out of the tree (`-deleted_size`
//! for a file, the walked subtree for a directory), but restoring from the trash
//! committed the entry back without ever adjusting `repo.size` — so a
//! delete/restore round trip lost the bytes from the owner's usage for good.
//! Two structural cases are pinned here as well:
//!
//! * a library whose last file was deleted has `EMPTY_SHA1` as its head root,
//!   which used to make every restore fail with `Directory / not found.` even
//!   though the root is simply empty;
//! * the same-name guard needs the *root* directory's entries too, otherwise a
//!   `/`-level restore silently pushed a second dirent with the name it was
//!   supposed to reject.

mod common;

use common::TestFixture;
use sea_orm::EntityTrait;
use serde_json::{Value, json};

/// The value `compute_user_usage` sums into the owner's usage.
async fn repo_size(f: &TestFixture) -> i64 {
    infra::entity::repo::Entity::find_by_id(f.repo_id.clone())
        .one(&*f.server.db)
        .await
        .unwrap()
        .expect("repo row")
        .size
}

async fn account_usage(f: &TestFixture) -> i64 {
    let resp = f
        .client
        .get("/api2/account/info/", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    body["usage"].as_i64().unwrap()
}

/// `(name, size)` of every entry of a directory, sorted by name. Duplicates are
/// preserved: a tree must never hold two dirents with the same name.
async fn listing(f: &TestFixture, path: &str) -> Vec<(String, i64)> {
    let resp = f.client.list_dir(&f.api_token, &f.repo_id, path).await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let mut out: Vec<(String, i64)> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["name"].as_str().unwrap().to_string(),
                e["size"].as_i64().unwrap(),
            )
        })
        .collect();
    out.sort();
    out
}

/// The newest trash entry as `(commit_id, full path)`.
async fn newest_trash_entry(f: &TestFixture) -> (String, String) {
    let resp = f
        .client
        .get(
            &format!("/api/v2.1/repos/{}/trash2/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let item = &body["items"][0];
    let parent_dir = item["parent_dir"].as_str().unwrap();
    let obj_name = item["obj_name"].as_str().unwrap();
    let path = if parent_dir == "/" {
        format!("/{obj_name}")
    } else {
        format!("{parent_dir}/{obj_name}")
    };
    (item["commit_id"].as_str().unwrap().to_string(), path)
}

/// Restore one path through `/trash2/revert/`; returns the response body.
async fn revert(f: &TestFixture, commit_id: &str, path: &str) -> Value {
    let mut map = serde_json::Map::new();
    map.insert(commit_id.to_string(), json!([path]));
    let resp = f
        .client
        .post_json(
            &format!("/api/v2.1/repos/{}/trash2/revert/", f.repo_id),
            Some(&f.api_token),
            &Value::Object(map),
        )
        .await;
    assert_eq!(resp.status(), 200);
    resp.json().await.unwrap()
}

/// Delete one file or directory through the REST API (which is what records the
/// trash entry).
async fn delete(f: &TestFixture, kind: &str, path: &str) {
    let resp = f
        .client
        .delete(
            &format!("/api2/repos/{}/{}/?p={}", f.repo_id, kind, path),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200, "delete {path} failed");
}

async fn upload(f: &TestFixture, dir: &str, name: &str, data: &[u8]) {
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, dir, name, data)
        .await;
    assert_eq!(resp.status(), 200, "upload {dir}/{name} failed");
}

/// A restore that reports success must not stay silent about the bytes it put
/// back.
#[tokio::test]
async fn restore_file_adds_its_size_back() {
    let f = TestFixture::new().await;
    upload(&f, "/", "a.txt", b"0123456789").await;
    upload(&f, "/", "b.txt", b"01234").await;
    assert_eq!(repo_size(&f).await, 15);

    delete(&f, "file", "/a.txt").await;
    assert_eq!(repo_size(&f).await, 5);

    let (commit_id, path) = newest_trash_entry(&f).await;
    assert_eq!(path, "/a.txt");
    let body = revert(&f, &commit_id, &path).await;
    assert_eq!(body["success"].as_array().unwrap().len(), 1, "{body}");

    assert_eq!(
        repo_size(&f).await,
        15,
        "restoring a file must add its bytes back to the library"
    );
    assert_eq!(account_usage(&f).await, 15);
    assert_eq!(
        listing(&f, "/").await,
        vec![("a.txt".to_string(), 10), ("b.txt".to_string(), 5)]
    );
}

/// A directory dirent carries no size of its own, so the deleted subtree is only
/// recoverable by walking the restored object.
#[tokio::test]
async fn restore_directory_adds_its_subtree_back() {
    let f = TestFixture::new().await;
    upload(&f, "/", "keep.txt", b"01234").await;
    let resp = f.client.create_dir(&f.api_token, &f.repo_id, "/d").await;
    assert_eq!(resp.status(), 200);
    let resp = f
        .client
        .create_dir(&f.api_token, &f.repo_id, "/d/sub")
        .await;
    assert_eq!(resp.status(), 200);
    upload(&f, "/d", "x.txt", b"0123456789").await;
    upload(&f, "/d/sub", "y.txt", &[b'y'; 20]).await;
    assert_eq!(repo_size(&f).await, 35);

    delete(&f, "dir", "/d").await;
    assert_eq!(repo_size(&f).await, 5);

    let (commit_id, path) = newest_trash_entry(&f).await;
    assert_eq!(path, "/d");
    let body = revert(&f, &commit_id, &path).await;
    assert_eq!(body["success"].as_array().unwrap().len(), 1, "{body}");

    assert_eq!(
        repo_size(&f).await,
        35,
        "restoring a directory must add its whole subtree back"
    );
    assert_eq!(account_usage(&f).await, 35);
}

/// The nested-parent path ("/d") resolves the same way as the root case.
#[tokio::test]
async fn restore_nested_file_adds_its_size_back() {
    let f = TestFixture::new().await;
    upload(&f, "/", "keep.txt", b"01234").await;
    let resp = f.client.create_dir(&f.api_token, &f.repo_id, "/d").await;
    assert_eq!(resp.status(), 200);
    upload(&f, "/d", "x.txt", b"0123456789").await;
    assert_eq!(repo_size(&f).await, 15);

    delete(&f, "file", "/d/x.txt").await;
    assert_eq!(repo_size(&f).await, 5);

    let (commit_id, path) = newest_trash_entry(&f).await;
    assert_eq!(path, "/d/x.txt");
    revert(&f, &commit_id, &path).await;

    assert_eq!(repo_size(&f).await, 15);
    assert_eq!(listing(&f, "/d").await, vec![("x.txt".to_string(), 10)]);
}

/// Deleting a library's last file leaves `EMPTY_SHA1` as the head root; the root
/// is empty, not missing, so restoring into it has to work.
#[tokio::test]
async fn restore_into_an_emptied_library_works() {
    let f = TestFixture::new().await;
    upload(&f, "/", "a.txt", b"0123456789").await;
    delete(&f, "file", "/a.txt").await;
    assert_eq!(repo_size(&f).await, 0);

    let (commit_id, path) = newest_trash_entry(&f).await;
    let body = revert(&f, &commit_id, &path).await;
    assert_eq!(
        body["success"].as_array().unwrap().len(),
        1,
        "the root of an emptied library exists, it is just empty: {body}"
    );

    assert_eq!(repo_size(&f).await, 10);
    assert_eq!(account_usage(&f).await, 10);
    assert_eq!(listing(&f, "/").await, vec![("a.txt".to_string(), 10)]);
}

/// The same-name guard was blind to `/`-level restores, so a restore could plant
/// a second dirent with an existing name.
#[tokio::test]
async fn restore_over_an_existing_name_is_rejected() {
    let f = TestFixture::new().await;
    upload(&f, "/", "a.txt", b"0123456789").await;
    upload(&f, "/", "b.txt", b"01234").await;
    delete(&f, "file", "/a.txt").await;

    // A new file takes the name back, then the trashed one is restored.
    upload(&f, "/", "a.txt", b"0123456789abcde").await;
    assert_eq!(repo_size(&f).await, 20);

    let (commit_id, path) = newest_trash_entry(&f).await;
    let body = revert(&f, &commit_id, &path).await;
    let failed = body["failed"].as_array().unwrap();
    assert_eq!(failed.len(), 1, "{body}");
    assert!(
        failed[0]["error_msg"]
            .as_str()
            .unwrap()
            .contains("already exists"),
        "{body}"
    );
    assert_eq!(
        repo_size(&f).await,
        20,
        "a rejected restore changes nothing"
    );
    assert_eq!(
        listing(&f, "/").await,
        vec![("a.txt".to_string(), 15), ("b.txt".to_string(), 5)],
        "a rejected restore must not leave a duplicate dirent behind"
    );
}

/// The older `revert-dirents` endpoint funnels into the same restore path.
#[tokio::test]
async fn restore_through_the_older_endpoint_adds_its_size_back() {
    let f = TestFixture::new().await;
    upload(&f, "/", "a.txt", b"0123456789").await;
    upload(&f, "/", "b.txt", b"01234").await;
    delete(&f, "file", "/a.txt").await;
    assert_eq!(repo_size(&f).await, 5);

    let (commit_id, _) = newest_trash_entry(&f).await;
    let resp = f
        .client
        .post_form(
            &format!("/api/v2.1/repos/{}/trash/revert-dirents/", f.repo_id),
            Some(&f.api_token),
            &[("commit_id", commit_id.as_str()), ("file_names", "a.txt")],
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["success"].as_array().unwrap().len(), 1, "{body}");

    assert_eq!(repo_size(&f).await, 15);
    assert_eq!(account_usage(&f).await, 15);
}
