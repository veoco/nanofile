//! Library trash: deleting a library keeps its content restorable, and purging
//! it frees the space.
//!
//! Deleting a library removes its `repos` row, which cascades into `commits` and
//! `fs_objects`, while garbage collection deliberately keeps the library's block
//! directory as long as the trash entry exists. Only the archive tables can
//! enumerate those blocks again, so these tests cover the whole cycle through
//! the public API: delete, list, restore, purge.

mod common;

use common::{TestFixture, create_test_user};
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use serde_json::Value;
use server::fs::core::GcManager;

/// `GET /api/v2.1/deleted-repos/` — a bare JSON array, as seahub returns.
async fn deleted_repos(f: &TestFixture) -> Vec<Value> {
    let resp = f
        .client
        .get("/api/v2.1/deleted-repos/", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200, "listing the trash failed");
    let body: Value = resp.json().await.unwrap();
    body.as_array()
        .expect("deleted-repos must be a JSON array")
        .clone()
}

/// The block directory of a library, as the store itself reports it.
async fn repo_dir_exists(store: &infra::storage::DynBlockStorage, repo_id: &str) -> bool {
    store
        .repo_dirs()
        .await
        .unwrap()
        .iter()
        .any(|(id, _)| id == repo_id)
}

/// Upload a file and return the repo's head commit after the upload.
async fn upload_and_head(f: &TestFixture, name: &str, content: &[u8]) -> Value {
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", name, content)
        .await;
    assert!(resp.status().is_success(), "upload failed");

    let repo: Value = f
        .client
        .get_repo(&f.api_token, &f.repo_id)
        .await
        .json()
        .await
        .unwrap();
    assert!(
        repo["head_commit_id"].is_string(),
        "the library must have a head commit after an upload"
    );
    repo["head_commit_id"].clone()
}

/// A cookie-authenticated Web UI client, the way the browser behaves.
async fn ui_login(server: &common::TestServer) -> reqwest::Client {
    let client = reqwest::Client::builder()
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let resp = client
        .post(format!("{}/accounts/login/", server.base_url))
        .form(&[("email", "test@example.com"), ("password", "password")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 302, "UI login should redirect");

    // Trigger a GET so the Set-Cookie headers are stored.
    let _ = client
        .get(format!("{}/libraries/", server.base_url))
        .send()
        .await;
    client
}

/// The trash page has a Deleted Libraries tab that lists the trashed library and
/// offers both actions, and the Files tab is the one shown by default.
#[tokio::test]
async fn trash_page_lists_deleted_libraries() {
    let f = TestFixture::new().await;
    let ui = ui_login(&f.server).await;

    // Nothing deleted yet: the tab exists and reports an empty trash.
    let html = ui
        .get(format!("{}/trash/?tab=libraries", f.server.base_url))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("data-tab=\"files\""), "files tab missing");
    assert!(
        html.contains("data-tab=\"libraries\""),
        "libraries tab missing"
    );
    assert!(
        html.contains("No deleted libraries."),
        "the empty libraries tab must say so"
    );

    upload_and_head(&f, "ui.txt", b"ui").await;
    let resp = f.client.delete_repo(&f.api_token, &f.repo_id).await;
    assert!(resp.status().is_success());

    // The default page opens on the files tab.
    let default_page = ui
        .get(format!("{}/trash/", f.server.base_url))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        default_page.contains("id=\"tab-libraries\" class=\"tab-content hidden\""),
        "the libraries tab must start hidden"
    );

    let html = ui
        .get(format!("{}/trash/?tab=libraries", f.server.base_url))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        !html.contains("id=\"tab-libraries\" class=\"tab-content hidden\""),
        "?tab=libraries must open that tab"
    );
    assert!(html.contains("test-repo"), "the deleted library is listed");
    assert!(html.contains(&format!("data-repo-id=\"{}\"", f.repo_id)));
    assert!(html.contains("data-action=\"restore-lib\""));
    assert!(html.contains("data-action=\"delete-lib\""));
    assert!(html.contains("data-action=\"delete-all-libs\""));
}

/// The whole point of the trash: a library that is deleted and restored comes
/// back with its files, its history and its head commit.
#[tokio::test]
async fn restore_brings_the_library_content_back() {
    let f = TestFixture::new().await;
    let content: Vec<u8> = (0..4096u32).map(|i| (i % 241) as u8).collect();
    let head_before = upload_and_head(&f, "keep.txt", &content).await;

    let resp = f.client.delete_repo(&f.api_token, &f.repo_id).await;
    assert!(
        resp.status().is_success(),
        "delete failed: {}",
        resp.status()
    );

    // Listed in the trash with the head commit the client knows about.
    let trash = deleted_repos(&f).await;
    let entry = trash
        .iter()
        .find(|e| e["repo_id"] == f.repo_id.as_str())
        .expect("the deleted library must be listed in the trash");
    assert_eq!(entry["head_commit_id"], head_before);
    assert_eq!(entry["repo_name"], "test-repo");

    // A trashed library is not readable.
    let resp = f.client.get_repo(&f.api_token, &f.repo_id).await;
    assert_eq!(resp.status(), 404);

    // seahub posts the restore as a form.
    let resp = f
        .client
        .post_form(
            "/api/v2.1/deleted-repos/",
            Some(&f.api_token),
            &[("repo_id", f.repo_id.as_str())],
        )
        .await;
    assert_eq!(resp.status(), 200, "restore failed");
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["success"], true);
    assert!(
        deleted_repos(&f).await.is_empty(),
        "the trash entry is gone"
    );

    // Content and head are back.
    let resp = f
        .client
        .download_file(&f.api_token, &f.repo_id, "/keep.txt")
        .await;
    assert_eq!(resp.status(), 200, "download after restore");
    assert_eq!(resp.bytes().await.unwrap().to_vec(), content);

    let repo: Value = f
        .client
        .get_repo(&f.api_token, &f.repo_id)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(repo["head_commit_id"], head_before);

    let listing: Value = f
        .client
        .list_dir(&f.api_token, &f.repo_id, "/")
        .await
        .json()
        .await
        .unwrap();
    assert!(
        listing.to_string().contains("keep.txt"),
        "the restored library must list its file: {listing}"
    );
}

/// The old JSON body still works, the new form body is what seahub sends, and
/// an empty repo id is rejected.
#[tokio::test]
async fn restore_accepts_form_and_json_bodies() {
    let f = TestFixture::new().await;
    let content = b"body shapes".to_vec();
    upload_and_head(&f, "bodies.txt", &content).await;

    let resp = f.client.delete_repo(&f.api_token, &f.repo_id).await;
    assert!(resp.status().is_success());

    // An empty repo id is a bad request, not a panic or a 500.
    let resp = f
        .client
        .post_form(
            "/api/v2.1/deleted-repos/",
            Some(&f.api_token),
            &[("repo_id", "")],
        )
        .await;
    assert_eq!(resp.status(), 400);

    let resp = f
        .client
        .post_json(
            "/api/v2.1/deleted-repos/",
            Some(&f.api_token),
            &serde_json::json!({"repo_id": &f.repo_id}),
        )
        .await;
    assert_eq!(resp.status(), 200, "JSON restore failed");
    assert_eq!(
        deleted_repos(&f).await.len(),
        0,
        "the JSON body must be accepted and the library restored"
    );
}

/// While a library is in the trash its blocks are kept, so restoring it later
/// still serves the files — and the next garbage collection leaves them alone.
#[tokio::test]
async fn trashed_library_blocks_survive_gc_and_come_back() {
    let f = TestFixture::new().await;
    let content: Vec<u8> = (0..8192u32).map(|i| (i % 251) as u8).collect();
    upload_and_head(&f, "gc.txt", &content).await;

    let store = infra::storage::new_block_store(&f.server.block_dir);
    assert!(repo_dir_exists(&store, &f.repo_id).await);

    let resp = f.client.delete_repo(&f.api_token, &f.repo_id).await;
    assert!(resp.status().is_success());

    // GC must not reclaim the blocks of a restorable library...
    let removed = GcManager::garbage_collect(&f.server.repos, &store)
        .await
        .unwrap();
    assert_eq!(removed, 0, "a trashed library's blocks must be kept");
    assert!(repo_dir_exists(&store, &f.repo_id).await);

    // ... which is what makes the restore able to serve them.
    let resp = f
        .client
        .post_form(
            "/api/v2.1/deleted-repos/",
            Some(&f.api_token),
            &[("repo_id", f.repo_id.as_str())],
        )
        .await;
    assert_eq!(resp.status(), 200);

    let resp = f
        .client
        .download_file(&f.api_token, &f.repo_id, "/gc.txt")
        .await;
    assert_eq!(resp.status(), 200, "download after restore");
    assert_eq!(resp.bytes().await.unwrap().to_vec(), content);

    // A restored library is live content again.
    let removed = GcManager::garbage_collect(&f.server.repos, &store)
        .await
        .unwrap();
    assert_eq!(removed, 0, "a restored library's blocks are reachable");
}

/// Purging a trashed library throws its content away and frees its blocks.
#[tokio::test]
async fn purging_a_library_frees_its_blocks() {
    let f = TestFixture::new().await;
    let content = b"purge me".to_vec();
    upload_and_head(&f, "purged.txt", &content).await;

    let store = infra::storage::new_block_store(&f.server.block_dir);
    assert!(repo_dir_exists(&store, &f.repo_id).await);

    let resp = f.client.delete_repo(&f.api_token, &f.repo_id).await;
    assert!(resp.status().is_success());
    assert!(
        repo_dir_exists(&store, &f.repo_id).await,
        "the blocks stay while the library is restorable"
    );

    let resp = f
        .client
        .delete(
            &format!("/api/v2.1/deleted-repos/{}/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200, "purge failed");
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["success"], true);

    assert!(
        !repo_dir_exists(&store, &f.repo_id).await,
        "purging must reclaim the library's blocks"
    );
    assert!(deleted_repos(&f).await.is_empty());

    let resp = f.client.get_repo(&f.api_token, &f.repo_id).await;
    assert_eq!(resp.status(), 404, "a purged library is gone for good");

    // Purging it again is a 404, not an error.
    let resp = f
        .client
        .delete(
            &format!("/api/v2.1/deleted-repos/{}/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 404);
}

/// `DELETE /api/v2.1/deleted-repos/` empties the whole trash.
#[tokio::test]
async fn emptying_the_trash_purges_every_library() {
    let f = TestFixture::new().await;
    upload_and_head(&f, "first.txt", b"first").await;

    let resp = f.client.create_repo(&f.api_token, "second-repo").await;
    assert!(resp.status().is_success(), "create repo failed");
    let created: Value = resp.json().await.unwrap();
    let second_id = created["repo_id"].as_str().unwrap().to_string();

    for repo_id in [&f.repo_id, &second_id] {
        let resp = f.client.delete_repo(&f.api_token, repo_id).await;
        assert!(resp.status().is_success(), "delete {repo_id} failed");
    }
    assert_eq!(deleted_repos(&f).await.len(), 2);

    let resp = f
        .client
        .delete("/api/v2.1/deleted-repos/", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200, "emptying the trash failed");

    assert!(deleted_repos(&f).await.is_empty());
    let store = infra::storage::new_block_store(&f.server.block_dir);
    assert!(!repo_dir_exists(&store, &f.repo_id).await);
    assert!(!repo_dir_exists(&store, &second_id).await);
}

/// Restoring and purging are owner-only, and one user's sweep cannot touch
/// another user's trash.
#[tokio::test]
async fn trash_operations_are_owner_scoped() {
    let f = TestFixture::new().await;
    let content = b"owned".to_vec();
    upload_and_head(&f, "owned.txt", &content).await;

    let resp = f.client.delete_repo(&f.api_token, &f.repo_id).await;
    assert!(resp.status().is_success());

    create_test_user(f.server.db.as_ref(), "other-trash@test.com", "password").await;
    let resp = f.client.login("other-trash@test.com", "password").await;
    let login: Value = resp.json().await.unwrap();
    let other_token = login["token"].as_str().unwrap().to_string();

    // A stranger cannot restore or purge the library...
    let resp = f
        .client
        .post_form(
            "/api/v2.1/deleted-repos/",
            Some(&other_token),
            &[("repo_id", f.repo_id.as_str())],
        )
        .await;
    assert_eq!(resp.status(), 403);

    let resp = f
        .client
        .delete(
            &format!("/api/v2.1/deleted-repos/{}/", f.repo_id),
            Some(&other_token),
        )
        .await;
    assert_eq!(resp.status(), 403);

    // ... and their own trash is empty, so emptying it touches nothing.
    assert_eq!(
        deleted_repos(&f).await.len(),
        1,
        "the owner's trash is intact"
    );
    let resp = f
        .client
        .delete("/api/v2.1/deleted-repos/", Some(&other_token))
        .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        deleted_repos(&f).await.len(),
        1,
        "one user's sweep must not purge another user's library"
    );

    // The owner can still restore it, content included.
    let resp = f
        .client
        .post_form(
            "/api/v2.1/deleted-repos/",
            Some(&f.api_token),
            &[("repo_id", f.repo_id.as_str())],
        )
        .await;
    assert_eq!(resp.status(), 200);

    let resp = f
        .client
        .download_file(&f.api_token, &f.repo_id, "/owned.txt")
        .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.bytes().await.unwrap().to_vec(), content);
}

/// seahub's status codes and message for a library that is not in the trash:
/// 400 on a restore, 404 on a permanent delete.
#[tokio::test]
async fn missing_trash_entry_reports_seahub_status_codes() {
    let f = TestFixture::new().await;
    let unknown = "11111111-2222-3333-4444-555555555555";

    let resp = f
        .client
        .post_form(
            "/api/v2.1/deleted-repos/",
            Some(&f.api_token),
            &[("repo_id", unknown)],
        )
        .await;
    assert_eq!(resp.status(), 400, "restore of an unknown library");
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error_msg"], "Library does not exist in trash.");

    let resp = f
        .client
        .delete(
            &format!("/api/v2.1/deleted-repos/{unknown}/"),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 404, "permanent delete of an unknown library");
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error_msg"], "Library does not exist in trash.");
}

/// A trash entry from an installation that predates the archive still restores,
/// but the library comes back empty: its content was destroyed when it was
/// deleted, and that is reported in the log rather than hidden.
#[tokio::test]
async fn restore_without_archived_content_comes_back_empty() {
    let f = TestFixture::new().await;
    upload_and_head(&f, "gone.txt", b"gone").await;

    let resp = f.client.delete_repo(&f.api_token, &f.repo_id).await;
    assert!(resp.status().is_success());

    // Simulate the old behaviour: the trash entry exists, the archive does not.
    for table in ["deleted_repo_commits", "deleted_repo_fs_objects"] {
        f.server
            .db
            .execute_raw(Statement::from_string(
                DatabaseBackend::Sqlite,
                format!("DELETE FROM {table}"),
            ))
            .await
            .unwrap();
    }

    let resp = f
        .client
        .post_form(
            "/api/v2.1/deleted-repos/",
            Some(&f.api_token),
            &[("repo_id", f.repo_id.as_str())],
        )
        .await;
    assert_eq!(resp.status(), 200, "the restore itself must still succeed");

    let repo: Value = f
        .client
        .get_repo(&f.api_token, &f.repo_id)
        .await
        .json()
        .await
        .unwrap();
    assert!(
        repo["head_commit_id"].is_null(),
        "there is no commit left to restore"
    );

    // The file cannot come back: its commit and FS objects are gone.
    let link = f
        .client
        .get(
            &format!("/api2/repos/{}/file/?p=/gone.txt", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert!(
        !link.status().is_success(),
        "a restored-but-empty library must not hand out a download link"
    );
    assert!(deleted_repos(&f).await.is_empty());
}
