//! Parity between the four delete entrypoints.
//!
//! The v1 file delete, v1 directory delete, WebDAV/v2.1 dirent delete and batch
//! delete used to own separate copies of the delete sequence; they had drifted on
//! tag cleanup and on whether a missing target was an error. These tests delete the
//! same shape of tree through each entrypoint and assert the same observable state.

use crate::common::TestFixture;

use infra::entity::{file_tag, file_trash};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

fn metadata_base(repo_id: &str) -> String {
    format!("/api/v2.1/repos/{repo_id}/metadata/")
}

async fn file_tags(f: &TestFixture) -> usize {
    file_tag::Entity::find()
        .filter(file_tag::Column::RepoId.eq(&f.repo_id))
        .all(&*f.server.db)
        .await
        .expect("read file tags")
        .len()
}

async fn trash_rows(f: &TestFixture, obj_name: &str) -> usize {
    file_trash::Entity::find()
        .filter(file_trash::Column::RepoId.eq(&f.repo_id))
        .filter(file_trash::Column::ObjName.eq(obj_name))
        .all(&*f.server.db)
        .await
        .expect("read file trash")
        .len()
}

/// Create a repo tag and attach it to one file, so a delete must clean it up.
async fn tag_file(f: &TestFixture, parent_dir: &str, name: &str) {
    let resp = f
        .client
        .post_json(
            &format!("{}tags/", metadata_base(&f.repo_id)),
            Some(&f.api_token),
            &serde_json::json!({
                "tags_data": [{ "_tag_name": "delete-parity", "_tag_color": "#ff9800" }],
            }),
        )
        .await;
    assert_eq!(resp.status(), 200, "create repo tag failed");
    let tag_id = resp.json::<serde_json::Value>().await.unwrap()["tags"][0]["_id"]
        .as_str()
        .unwrap()
        .to_string();

    let record_url = format!(
        "{}record/?parent_dir={parent_dir}&name={name}&file_name={name}",
        metadata_base(&f.repo_id)
    );
    let resp = f.client.get(&record_url, Some(&f.api_token)).await;
    assert_eq!(resp.status(), 200, "resolve file record failed");
    let record_id = resp.json::<serde_json::Value>().await.unwrap()["results"][0]["_id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = f
        .client
        .put_json(
            &format!("{}file-tags/", metadata_base(&f.repo_id)),
            Some(&f.api_token),
            &serde_json::json!({
                "file_tags_data": [{ "record_id": record_id, "tags": [tag_id] }],
            }),
        )
        .await;
    assert_eq!(resp.status(), 200, "attach file tag failed");
    assert_eq!(file_tags(f).await, 1);
}

async fn create_dir(f: &TestFixture, path: &str) {
    let resp = f.client.create_dir(&f.api_token, &f.repo_id, path).await;
    assert_eq!(resp.status(), 201, "mkdir {path} failed");
}

async fn upload(f: &TestFixture, parent_dir: &str, name: &str, body: &[u8]) {
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, parent_dir, name, body)
        .await;
    assert_eq!(resp.status(), 200, "upload {parent_dir}/{name} failed");
}

async fn list_names(f: &TestFixture, path: &str) -> Vec<String> {
    let resp = f.client.list_dir(&f.api_token, &f.repo_id, path).await;
    assert_eq!(resp.status(), 200, "list {path} failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    body.as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|e| e["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// v2 `DELETE /api2/repos/{id}/file/` cleans tags and records the trash row.
#[tokio::test]
async fn v2_file_delete_cleans_tags_and_records_trash() {
    let f = TestFixture::new().await;
    upload(&f, "/", "a.txt", b"content").await;
    tag_file(&f, "/", "a.txt").await;

    let resp = f
        .client
        .delete(
            &format!("/api2/repos/{}/file/?p=/a.txt", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200, "v2 file delete failed");
    assert!(!list_names(&f, "/").await.contains(&"a.txt".to_string()));
    assert_eq!(file_tags(&f).await, 0, "file tags must be cleaned");
    assert_eq!(trash_rows(&f, "a.txt").await, 1, "one trash row expected");
}

/// v2 `DELETE /api2/repos/{id}/dir/` cleans tags for the whole subtree.
#[tokio::test]
async fn v2_dir_delete_cleans_nested_tags_and_records_trash() {
    let f = TestFixture::new().await;
    create_dir(&f, "/dir").await;
    upload(&f, "/dir", "inner.txt", b"content").await;
    tag_file(&f, "/dir", "inner.txt").await;

    let resp = f
        .client
        .delete(
            &format!("/api2/repos/{}/dir/?p=/dir", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200, "v2 dir delete failed");
    assert!(!list_names(&f, "/").await.contains(&"dir".to_string()));
    assert_eq!(file_tags(&f).await, 0, "nested file tags must be cleaned");
    assert_eq!(trash_rows(&f, "dir").await, 1, "one trash row expected");
}

/// WebDAV reaches `delete_dirent`, which used to skip tag cleanup entirely.
#[tokio::test]
async fn webdav_dirent_delete_cleans_nested_tags() {
    let f = TestFixture::new().await;
    create_dir(&f, "/dir").await;
    upload(&f, "/dir", "inner.txt", b"content").await;
    tag_file(&f, "/dir", "inner.txt").await;

    f.server
        .state
        .dir_service()
        .delete_dirent(&f.repo_id, "dir", "/dir", &f.email, f.user_id)
        .await
        .expect("dirent delete failed");

    assert!(!list_names(&f, "/").await.contains(&"dir".to_string()));
    assert_eq!(file_tags(&f).await, 0, "nested file tags must be cleaned");
    assert_eq!(trash_rows(&f, "dir").await, 1, "one trash row expected");
}

/// The v2.1 batch endpoint deletes a directory subtree through `batch_delete`;
/// its tag cleanup used to be missing too.
#[tokio::test]
async fn batch_delete_cleans_nested_tags() {
    let f = TestFixture::new().await;
    create_dir(&f, "/dir").await;
    upload(&f, "/dir", "inner.txt", b"content").await;
    tag_file(&f, "/dir", "inner.txt").await;

    let resp = f
        .client
        .delete_json(
            "/api/v2.1/repos/batch-delete-item/",
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "parent_dir": "/",
                "dirents": ["dir"],
            }),
        )
        .await;
    assert_eq!(resp.status(), 200, "batch delete failed");
    assert!(!list_names(&f, "/").await.contains(&"dir".to_string()));
    assert_eq!(file_tags(&f).await, 0, "nested file tags must be cleaned");
    assert_eq!(trash_rows(&f, "dir").await, 1, "one trash row expected");
}
