//! Integration tests for the full-text search indexer.
//!
//! These tests verify that file operations (upload, delete, rename, move)
//! correctly update the Tantivy full-text search index.
//!
//! All tests use `TestFixture::new_with_index()` which starts the server
//! with the indexer enabled on a temporary index directory.

mod common;

/// Perform a full-text content search and return the `results` array.
async fn search_results(f: &common::TestFixture, token: &str, q: &str) -> Vec<serde_json::Value> {
    let resp = f
        .client
        .get(
            &format!("/api2/search/?q={q}&search_filename_only=false"),
            Some(token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    resp.json::<serde_json::Value>().await.unwrap()["results"]
        .as_array()
        .unwrap()
        .clone()
}

/// Poll `predicate` until it returns true or `timeout` elapses. Returns `true`
/// if the predicate became true, `false` on timeout.
///
/// The indexer commits asynchronously (debounced), so a freshly uploaded,
/// renamed, moved or deleted file is not searchable immediately. Polling
/// instead of sleeping a fixed duration keeps these tests robust on slow CI
/// runners where the debounced commit (and Tantivy's first-commit cold start)
/// can exceed a fixed delay.
async fn wait_for<F, Fut>(timeout: std::time::Duration, mut predicate: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if predicate().await {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

/// Upload a text file, then search for content → should find it.
#[tokio::test]
async fn test_upload_and_search_content() {
    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;

    // Upload a text file with unique content
    let resp = f
        .client
        .upload_file(
            token,
            &f.repo_id,
            "/",
            "hello.txt",
            b"hello world from nanofile",
        )
        .await;
    assert_eq!(resp.status(), 200, "upload should succeed");

    // Give the indexer time to commit (poll, not fixed sleep)
    assert!(
        wait_for(std::time::Duration::from_secs(15), || async {
            let results = search_results(&f, token, "nanofile").await;
            !results.is_empty()
        })
        .await,
        "should find file via content search"
    );

    // Search for content (not filename) — should find the file
    let results = search_results(&f, token, "nanofile").await;
    assert_eq!(results[0]["name"], "hello.txt");
    assert_eq!(results[0]["fullpath"], "/hello.txt");
}

/// Search for content that does NOT match the filename (only in content).
#[tokio::test]
async fn test_search_content_not_filename() {
    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;

    // File with content that doesn't match its name
    let resp = f
        .client
        .upload_file(
            token,
            &f.repo_id,
            "/",
            "readme.md",
            b"installation guide for the software",
        )
        .await;
    assert_eq!(resp.status(), 200);

    // Full-text search should find it (content matches) — poll for the commit
    assert!(
        wait_for(std::time::Duration::from_secs(15), || async {
            let results = search_results(&f, token, "installation").await;
            !results.is_empty()
        })
        .await,
        "full-text search should find 'installation' in content"
    );

    // Filename-only search should NOT find it (name doesn't contain 'installation')
    let resp = f
        .client
        .get(
            "/api2/search/?q=installation&search_filename_only=true",
            Some(token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let results = body["results"].as_array().unwrap();
    assert!(
        results.is_empty(),
        "filename-only search should not find 'installation'"
    );
}

/// Upload a binary file → should NOT be indexed.
#[tokio::test]
async fn test_binary_file_skipped() {
    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;

    let resp = f
        .client
        .upload_file(
            token,
            &f.repo_id,
            "/",
            "image.png",
            b"\x89PNG\r\n\x1a\nbinary data",
        )
        .await;
    assert_eq!(resp.status(), 200);

    // Upload a text file as a commit signal: once it is searchable, the
    // debounced commit has definitely run, so the binary file's absence from
    // the index is meaningful (not just a pre-commit artifact).
    let resp = f
        .client
        .upload_file(token, &f.repo_id, "/", "signal.txt", b"commit signal")
        .await;
    assert_eq!(resp.status(), 200);
    assert!(
        wait_for(std::time::Duration::from_secs(15), || async {
            let results = search_results(&f, token, "signal").await;
            !results.is_empty()
        })
        .await,
        "text signal file should be indexed"
    );

    // Search for content in binary file — should not find it.
    let results = search_results(&f, token, "binary").await;
    assert!(results.is_empty(), "binary file should not be indexed");
}

/// Delete a file → should remove it from the index.
#[tokio::test]
async fn test_delete_cleans_index() {
    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;

    // Upload
    let resp = f
        .client
        .upload_file(
            token,
            &f.repo_id,
            "/",
            "delete_me.txt",
            b"content to delete",
        )
        .await;
    assert_eq!(resp.status(), 200);

    // Verify it's in the index (poll for the debounced commit)
    assert!(
        wait_for(std::time::Duration::from_secs(15), || async {
            let results = search_results(&f, token, "delete").await;
            !results.is_empty()
        })
        .await,
        "file should be in index before delete"
    );

    // Delete
    let resp = f
        .client
        .delete(
            &format!("/api2/repos/{}/file/?p=/delete_me.txt", f.repo_id),
            Some(token),
        )
        .await;
    assert_eq!(resp.status(), 200);

    // Search again — should not find it (poll for the delete to commit)
    assert!(
        wait_for(std::time::Duration::from_secs(15), || async {
            let results = search_results(&f, token, "delete").await;
            results.is_empty()
        })
        .await,
        "file should be removed from index after deletion"
    );
}

/// Rename a file → new path should be searchable.
#[tokio::test]
async fn test_rename_updates_index() {
    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;

    // Upload
    let resp = f
        .client
        .upload_file(token, &f.repo_id, "/", "rename_me.txt", b"rename content")
        .await;
    assert_eq!(resp.status(), 200);

    // Rename via form POST
    let resp = f
        .client
        .post_form(
            &format!("/api2/repos/{}/file/?p=/rename_me.txt", f.repo_id),
            Some(token),
            &[("operation", "rename"), ("newname", "renamed.txt")],
        )
        .await;
    assert_eq!(resp.status(), 200);

    // Content search at new path should work (poll for the commit)
    assert!(
        wait_for(std::time::Duration::from_secs(15), || async {
            let results = search_results(&f, token, "rename+content").await;
            !results.is_empty()
        })
        .await,
        "should find renamed file via content"
    );
    let results = search_results(&f, token, "rename+content").await;
    assert_eq!(results[0]["name"], "renamed.txt");
}

/// Move a file to a subdirectory → new path should be searchable.
#[tokio::test]
async fn test_move_updates_index() {
    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;

    // Create a subdirectory
    let resp = f.client.create_dir(token, &f.repo_id, "/subdir").await;
    // seahub answers 201 Created for mkdir.
    assert_eq!(resp.status(), 201);

    // Upload
    let resp = f
        .client
        .upload_file(token, &f.repo_id, "/", "move_me.txt", b"move content test")
        .await;
    assert_eq!(resp.status(), 200);

    // Move via form POST
    let resp = f
        .client
        .post_form(
            &format!("/api2/repos/{}/file/?p=/move_me.txt", f.repo_id),
            Some(token),
            &[
                ("operation", "move"),
                ("dst_repo", &f.repo_id),
                ("dst_dir", "/subdir"),
            ],
        )
        .await;
    assert_eq!(resp.status(), 200);

    // Content search at new path should work (poll for the commit)
    assert!(
        wait_for(std::time::Duration::from_secs(15), || async {
            let results = search_results(&f, token, "move+content").await;
            !results.is_empty()
        })
        .await,
        "should find moved file via content"
    );
    let results = search_results(&f, token, "move+content").await;
    assert_eq!(results[0]["fullpath"], "/subdir/move_me.txt");
}

/// A rename performed through the sync protocol (`PUT /commit/HEAD`) must move
/// the index entry: the new path is searchable and the old path is gone.
///
/// The sync path deletes the old path via the `superseded` delete change that
/// activity logging now skips, so this guards the indexer half of that split.
#[tokio::test]
async fn test_sync_rename_updates_index() {
    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;
    let content = b"syncrenameuniqueword";

    let c1 = common::sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[("idx_old.txt", content, 0o100644)],
        &[],
        None,
    )
    .await;
    // Same object id, new name, same directory → the diff reports a rename.
    common::sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[("idx_new.txt", content, 0o100644)],
        &[],
        Some(&c1),
    )
    .await;

    // One polling loop for both halves: the search endpoint is rate limited
    // (60/min), so poll at a 500ms interval rather than every 100ms.
    let mut searchable_at_new_path = false;
    let mut gone_from_old_path = false;
    for _ in 0..30 {
        let results = search_results(&f, token, "syncrenameuniqueword").await;
        searchable_at_new_path = results.iter().any(|r| r["fullpath"] == "/idx_new.txt");
        gone_from_old_path = !results.iter().any(|r| r["fullpath"] == "/idx_old.txt");
        if searchable_at_new_path && gone_from_old_path {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert!(
        searchable_at_new_path,
        "renamed file should be searchable at its new path"
    );
    assert!(
        gone_from_old_path,
        "old path must be removed from the index after a sync rename"
    );
}

/// Poll `search_results` until `gone` is absent and every path in `want` is
/// present, or fail.
///
/// The search endpoint is rate limited (60/min), so this runs one poll per test
/// rather than polling before and after the operation.
async fn wait_for_paths(
    f: &common::TestFixture,
    token: &str,
    q: &str,
    want: &[&str],
    gone: &[&str],
) {
    let ok = |results: &[serde_json::Value]| {
        want.iter()
            .all(|w| results.iter().any(|h| h["fullpath"].as_str() == Some(*w)))
            && gone
                .iter()
                .all(|g| !results.iter().any(|h| h["fullpath"].as_str() == Some(*g)))
    };
    for _ in 0..20 {
        if ok(&search_results(f, token, q).await) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    let results = search_results(f, token, q).await;
    panic!("expected paths {want:?} (and not {gone:?}) for {q:?}; last results: {results:?}");
}

/// Renaming a *directory* via the web API must move the index entries of the
/// files inside it to the new path, and moving the directory must move them
/// again.
///
/// The directory rename path used to skip index updates entirely, which left
/// every file under a renamed directory unsearchable (the old path no longer
/// resolves in the FS tree, so the stale index hit was dropped).
#[tokio::test]
async fn test_dir_rename_and_move_update_index() {
    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;

    assert_eq!(
        f.client
            .create_dir(token, &f.repo_id, "/docs")
            .await
            .status(),
        201
    );
    assert_eq!(
        f.client
            .upload_file(token, &f.repo_id, "/docs", "note.txt", b"dirrenameword")
            .await
            .status(),
        200
    );

    // Rename the directory. `/docs/note.txt` is already indexed by the upload,
    // so a single poll after the rename proves the entry moved (not merely that
    // it became searchable).
    let resp = f
        .client
        .post_json(
            &format!("/api2/repos/{}/dir/rename/", f.repo_id),
            Some(token),
            &serde_json::json!({"repo_id": f.repo_id, "p": "/docs", "new_name": "manuals"}),
        )
        .await;
    assert_eq!(resp.status(), 200, "dir rename failed");
    wait_for_paths(
        &f,
        token,
        "dirrenameword",
        &["/manuals/note.txt"],
        &["/docs/note.txt"],
    )
    .await;

    // Move it under another directory.
    assert_eq!(
        f.client
            .create_dir(token, &f.repo_id, "/dst")
            .await
            .status(),
        201
    );
    let resp = f
        .client
        .post_json(
            &format!("/api2/repos/{}/dir/move/", f.repo_id),
            Some(token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "p": "/manuals",
                "new_parent_dir": "/dst",
            }),
        )
        .await;
    assert_eq!(resp.status(), 200, "dir move failed");
    wait_for_paths(
        &f,
        token,
        "dirrenameword",
        &["/dst/manuals/note.txt"],
        &["/manuals/note.txt"],
    )
    .await;
}

/// A sync client that renames a directory reports it as a single directory
/// event; the files inside must still move in the index.
#[tokio::test]
async fn test_sync_dir_rename_updates_index() {
    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;
    let content = b"syncdirrenameword";

    let c1 = common::sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[],
        &[("docs", &[("note.txt", content.as_slice(), 0o100644)])],
        None,
    )
    .await;

    common::sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[],
        &[("manuals", &[("note.txt", content.as_slice(), 0o100644)])],
        Some(&c1),
    )
    .await;

    wait_for_paths(
        &f,
        token,
        "syncdirrenameword",
        &["/manuals/note.txt"],
        &["/docs/note.txt"],
    )
    .await;
}

/// Deleting a directory must drop the index entries of every file inside it,
/// not just a document for the directory path itself.
#[tokio::test]
async fn test_dir_delete_cleans_index_subtree() {
    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;

    assert_eq!(
        f.client
            .create_dir(token, &f.repo_id, "/gone")
            .await
            .status(),
        201
    );
    assert_eq!(
        f.client
            .upload_file(token, &f.repo_id, "/gone", "note.txt", b"dirdelword")
            .await
            .status(),
        200
    );

    let resp = f
        .client
        .delete(
            &format!("/api2/repos/{}/dir/?p=/gone", f.repo_id),
            Some(token),
        )
        .await;
    assert_eq!(resp.status(), 200, "dir delete failed");

    wait_for_paths(&f, token, "dirdelword", &[], &["/gone/note.txt"]).await;
}

/// The same for a directory deleted by a sync client: the directory is reported
/// once, so the files under it must be cleared explicitly.
#[tokio::test]
async fn test_sync_dir_delete_cleans_index_subtree() {
    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;
    let content = b"syncdirdelword";

    let c1 = common::sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[],
        &[("gone", &[("note.txt", content.as_slice(), 0o100644)])],
        None,
    )
    .await;
    // Commit 2 drops the directory from the root.
    common::sync_commit(&f.client, &f.sync_token, &f.repo_id, &[], &[], Some(&c1)).await;

    wait_for_paths(&f, token, "syncdirdelword", &[], &["/gone/note.txt"]).await;
}

/// Batch delete → all files removed from index.
#[tokio::test]
async fn test_batch_delete_cleans_index() {
    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;

    // Upload multiple files
    for i in 0..3 {
        let name = format!("batch-{}.txt", i);
        let content = format!("batch content {}", i);
        let resp = f
            .client
            .upload_file(token, &f.repo_id, "/", &name, content.as_bytes())
            .await;
        assert_eq!(resp.status(), 200);
    }

    // Verify files are in index (poll for the debounced commit)
    assert!(
        wait_for(std::time::Duration::from_secs(15), || async {
            let results = search_results(&f, token, "batch").await;
            results.len() >= 3
        })
        .await,
        "should find batch files in index"
    );

    // Batch delete via v2.1 API
    let resp = f
        .client
        .post_json(
            "/api/v2.1/repos/batch-delete-item/",
            Some(token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "parent_dir": "/",
                "dirents": ["batch-0.txt", "batch-1.txt", "batch-2.txt"]
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);

    // Search again — should not find batch content (poll for the delete to
    // commit). Filenames might still match via the FS tree walk, so assert the
    // content term "batch content 0" is gone rather than an empty result set.
    assert!(
        wait_for(std::time::Duration::from_secs(15), || async {
            let results = search_results(&f, token, "batch+0").await;
            !results
                .iter()
                .any(|r| r["fullpath"].as_str().unwrap_or("").contains("batch-0.txt"))
        })
        .await,
        "batch delete should remove the content index entry"
    );
}

/// Multiple repos — content search should only find accessible files.
#[tokio::test]
async fn test_content_search_multi_repo() {
    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;

    // Upload content-matching file to first repo
    let resp = f
        .client
        .upload_file(token, &f.repo_id, "/", "unique.txt", b"unique term xyz987")
        .await;
    assert_eq!(resp.status(), 200);

    // Create second repo and upload with same content
    let repo2_id = common::create_test_repo(&f.client, token, "second-repo").await;
    let resp = f
        .client
        .upload_file(token, &repo2_id, "/", "other.txt", b"unique term xyz987")
        .await;
    assert_eq!(resp.status(), 200);

    // Search from first user — should find both repos (poll for the commit)
    assert!(
        wait_for(std::time::Duration::from_secs(15), || async {
            let results = search_results(&f, token, "xyz987").await;
            results.len() == 2
        })
        .await,
        "should find content in both repos for same user"
    );
}

/// Reindex endpoint should rebuild the index from storage.
#[tokio::test]
async fn test_reindex_endpoint() {
    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;

    // Upload a text file
    let resp = f
        .client
        .upload_file(
            token,
            &f.repo_id,
            "/",
            "reindexable.txt",
            b"reindexable content",
        )
        .await;
    assert_eq!(resp.status(), 200);

    // Verify it's indexed (poll for the debounced commit)
    assert!(
        wait_for(std::time::Duration::from_secs(15), || async {
            let results = search_results(&f, token, "reindexable").await;
            !results.is_empty()
        })
        .await,
        "file should be indexed before reindex"
    );

    // Call the reindex endpoint — starts a background task.
    let resp = f
        .client
        .post_json(
            "/api2/reindex/",
            Some(token),
            &serde_json::json!({"repo_id": f.repo_id}),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
    let task_id = body["task_id"].as_str().unwrap().to_string();

    // Poll until the background reindex task completes.
    let mut progress: serde_json::Value = serde_json::json!({});
    for _ in 0..100 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let resp = f
            .client
            .get(
                &format!("/api2/reindex-progress/?task_id={task_id}"),
                Some(token),
            )
            .await;
        assert_eq!(resp.status(), 200);
        progress = resp.json().await.unwrap();
        if progress["state"] == "completed" || progress["state"] == "failed" {
            break;
        }
    }
    assert_eq!(progress["state"], "completed");
    assert!(progress["indexed"].as_u64().unwrap() >= 1);
}

/// A reindex checks in before every file, so a busy server parks the pass
/// instead of letting it run to completion.
///
/// The catalog declares `chunkable` for this job. Without a checkpoint in the
/// body that declaration is a lie: the run is handed a gate and never uses it.
#[tokio::test]
async fn a_reindex_pass_parks_while_the_server_is_busy() {
    use server::tasks::admission::LoadThresholds;
    use server::tasks::run::{JobState, RunId};

    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;

    // Several files, so the pass has more than one boundary to stop at.
    for i in 0..8 {
        let resp = f
            .client
            .upload_file(
                token,
                &f.repo_id,
                "/",
                &format!("parkable-{i}.txt"),
                format!("parkable content number {i}").as_bytes(),
            )
            .await;
        assert_eq!(resp.status(), 200, "upload {i} should succeed");
    }

    // Load awareness on, with a threshold a single in-flight request exceeds.
    let tasks = &f.server.state.tasks;
    tasks.configure_load(true, 1);
    tasks.set_load_thresholds(LoadThresholds {
        max_inflight_requests: 0,
        max_db_utilization: 1.0,
        max_worker_busy_pct: 100,
    });

    // Hold the server busy for the whole pass.
    let busy = tasks.load().request_guard();

    let resp = f
        .client
        .post_json(
            "/api2/reindex/",
            Some(token),
            &serde_json::json!({"repo_id": f.repo_id}),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let task_id = resp.json::<serde_json::Value>().await.unwrap()["task_id"]
        .as_str()
        .unwrap()
        .to_string();
    let run_id = RunId::from_client(task_id);

    assert!(
        wait_for(std::time::Duration::from_secs(10), || async {
            tasks.store().get(&run_id).map(|run| run.state) == Some(JobState::Yielded)
        })
        .await,
        "a busy server must park the reindex pass"
    );

    // It stays parked: nothing advances while the load is still on.
    for _ in 0..5 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert_eq!(
            tasks.store().get(&run_id).map(|run| run.state),
            Some(JobState::Yielded)
        );
    }

    // The server goes quiet, and the pass finishes on its own.
    drop(busy);
    assert!(
        wait_for(std::time::Duration::from_secs(30), || async {
            tasks
                .store()
                .get(&run_id)
                .is_some_and(|run| run.state.is_terminal())
        })
        .await,
        "the pass must finish once the server is quiet"
    );

    let run = tasks.store().get(&run_id).unwrap();
    assert_eq!(run.state, JobState::Succeeded);
    // Parking held the pass back without making it skip work.
    assert_eq!(
        Some(run.progress.done),
        run.progress.total,
        "every file should still have been visited"
    );
    assert_eq!(run.progress.done, 8);
}

/// A pass stopped at a checkpoint ends as a cancellation, not as a failure.
///
/// The abandoned pass surfaces as an opaque error; the job body is what turns
/// it back into the terminal state the caller actually asked for.
#[tokio::test]
async fn a_parked_reindex_cancels_rather_than_fails() {
    use server::tasks::admission::LoadThresholds;
    use server::tasks::run::{JobState, RunId, Viewer};

    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;

    for i in 0..4 {
        let resp = f
            .client
            .upload_file(
                token,
                &f.repo_id,
                "/",
                &format!("cancel-{i}.txt"),
                format!("cancel content {i}").as_bytes(),
            )
            .await;
        assert_eq!(resp.status(), 200, "upload {i} should succeed");
    }

    let tasks = &f.server.state.tasks;
    tasks.configure_load(true, 1);
    tasks.set_load_thresholds(LoadThresholds {
        max_inflight_requests: 0,
        max_db_utilization: 1.0,
        max_worker_busy_pct: 100,
    });

    // The server stays busy for the whole test, so the pass can only end
    // because it was cancelled.
    let _busy = tasks.load().request_guard();

    let resp = f
        .client
        .post_json(
            "/api2/reindex/",
            Some(token),
            &serde_json::json!({"repo_id": f.repo_id}),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let task_id = resp.json::<serde_json::Value>().await.unwrap()["task_id"]
        .as_str()
        .unwrap()
        .to_string();
    let run_id = RunId::from_client(task_id);

    assert!(
        wait_for(std::time::Duration::from_secs(10), || async {
            tasks.store().get(&run_id).map(|run| run.state) == Some(JobState::Yielded)
        })
        .await,
        "a busy server must park the reindex pass"
    );

    tasks.cancel(&run_id, Viewer::user(f.user_id)).unwrap();
    assert!(
        wait_for(std::time::Duration::from_secs(10), || async {
            tasks
                .store()
                .get(&run_id)
                .is_some_and(|run| run.state.is_terminal())
        })
        .await,
        "a cancelled pass must reach a terminal state"
    );
    assert_eq!(
        tasks.store().get(&run_id).unwrap().state,
        JobState::Cancelled
    );
}

/// Upload a binary image, then use index_file_text to associate extracted text.
#[tokio::test]
async fn test_index_file_text_for_binary() {
    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;

    // Upload a binary file (PNG header) — not indexable as text.
    let resp = f
        .client
        .upload_file(
            token,
            &f.repo_id,
            "/",
            "screenshot.png",
            b"\x89PNG\r\n\x1a\nbinary pixel data here",
        )
        .await;
    assert_eq!(resp.status(), 200);

    // Upload a text signal file so we know the debounced commit has run before
    // asserting the binary file is absent from the index.
    let resp = f
        .client
        .upload_file(token, &f.repo_id, "/", "signal.txt", b"commit signal text")
        .await;
    assert_eq!(resp.status(), 200);
    assert!(
        wait_for(std::time::Duration::from_secs(15), || async {
            let results = search_results(&f, token, "signal").await;
            !results.is_empty()
        })
        .await,
        "signal file should be indexed"
    );

    // Verify the binary file is NOT found via content search.
    let results = search_results(&f, token, "vision+model").await;
    assert!(
        results.is_empty(),
        "binary file should not be indexed at upload"
    );

    // Now use index_file_text to associate extracted text with the image.
    let resp = f
        .client
        .post_json(
            "/api2/index-file-text/",
            Some(token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/screenshot.png",
                "text": "This screenshot shows the login page with username and password fields. The vision model extracted this text."
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");

    // Now search for the extracted text — should find the image (poll).
    assert!(
        wait_for(std::time::Duration::from_secs(15), || async {
            let results = search_results(&f, token, "vision+model+extracted").await;
            !results.is_empty() && results[0]["name"].as_str().unwrap_or("") == "screenshot.png"
        })
        .await,
        "should find image via custom text index"
    );

    // Update with different text — should replace the old index entry.
    let resp = f
        .client
        .post_json(
            "/api2/index-file-text/",
            Some(token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "path": "/screenshot.png",
                "text": "Updated text from a different vision model run."
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);

    // Old text should no longer match, new text should match (poll for both).
    assert!(
        wait_for(std::time::Duration::from_secs(15), || async {
            let old_results = search_results(&f, token, "login+page+username").await;
            let new_results = search_results(&f, token, "different+vision+model").await;
            old_results.is_empty()
                && !new_results.is_empty()
                && new_results[0]["name"].as_str().unwrap_or("") == "screenshot.png"
        })
        .await,
        "old text should be replaced by updated text"
    );
}

/// Prefix matching: search "case" should find file containing "Caseend".
#[tokio::test]
async fn test_prefix_matching_in_content() {
    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;

    // Upload a file with "Caseend" in the content
    let resp = f
        .client
        .upload_file(
            token,
            &f.repo_id,
            "/",
            "readme.md",
            b"This file mentions Caseend which is a compound word.",
        )
        .await;
    assert_eq!(resp.status(), 200);

    // Exact match should still work (poll for the debounced commit)
    assert!(
        wait_for(std::time::Duration::from_secs(15), || async {
            let results = search_results(&f, token, "Caseend").await;
            !results.is_empty()
        })
        .await,
        "exact match should find Caseend"
    );
    let results = search_results(&f, token, "Caseend").await;
    assert_eq!(results[0]["name"], "readme.md");

    // Prefix match: "case" should find "Caseend"
    let results = search_results(&f, token, "case").await;
    assert!(!results.is_empty(), "prefix 'case' should match 'Caseend'");
    assert_eq!(results[0]["name"], "readme.md");
}

/// Invalid query syntax (regex, field:foo, unbalanced quotes) should
/// return 200 with an empty result set, not a 500 error.
#[tokio::test]
async fn test_invalid_query_syntax_returns_empty_not_500() {
    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;

    // Upload a file so the index is non-empty.
    let resp = f
        .client
        .upload_file(token, &f.repo_id, "/", "doc.txt", b"some content here")
        .await;
    assert_eq!(resp.status(), 200);
    assert!(
        wait_for(std::time::Duration::from_secs(15), || async {
            let results = search_results(&f, token, "content").await;
            !results.is_empty()
        })
        .await,
        "file should be indexed"
    );

    // Various invalid/special syntaxes that used to return 500.
    for bad_query in &[
        "/.*a.*b.*/",   // regex syntax (disabled by default)
        "field:value",  // field-qualified query on non-existent field
        "\"unbalanced", // unbalanced quote
        "(a OR b",      // unbalanced paren
        "a~",           // fuzzy without valid distance
    ] {
        let resp = f
            .client
            .get(
                &format!("/api2/search/?q={bad_query}&search_filename_only=false"),
                Some(token),
            )
            .await;
        assert_eq!(
            resp.status(),
            200,
            "query {bad_query:?} should return 200, not 500"
        );
        let body: serde_json::Value = resp.json().await.unwrap();
        let results = body["results"].as_array().unwrap();
        assert!(
            results.is_empty(),
            "query {bad_query:?} should return empty results"
        );
    }
}

/// Overly long keyword should return 200 with empty results, not reach Tantivy.
#[tokio::test]
async fn test_overlong_keyword_returns_empty() {
    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;

    let long_query = "a".repeat(300); // exceeds MAX_KEYWORD_LEN (256)
    let resp = f
        .client
        .get(&format!("/api2/search/?q={long_query}"), Some(token))
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let results = body["results"].as_array().unwrap();
    assert!(results.is_empty(), "overlong keyword should return empty");
}

// ── Access control ─────────────────────────────────────────────────────

/// Fetch the `results` array for a search query, with an optional `search_repo`.
async fn search_results_with_repo(
    f: &common::TestFixture,
    token: &str,
    q: &str,
    search_repo: Option<&str>,
) -> Vec<serde_json::Value> {
    let mut url = format!("/api2/search/?q={q}&search_filename_only=false");
    if let Some(repo) = search_repo {
        url.push_str(&format!("&search_repo={repo}"));
    }
    let resp = f.client.get(&url, Some(token)).await;
    assert_eq!(resp.status(), 200, "search should return 200, url={url}");
    resp.json::<serde_json::Value>().await.unwrap()["results"]
        .as_array()
        .unwrap()
        .clone()
}

/// Regression: a user with no accessible repos must not be able to use
/// the full-text index to read another user's filenames or content. An empty
/// accessible-repo set used to be passed to the indexer as "no filter".
#[tokio::test]
async fn test_zero_repo_user_cannot_search_other_repos() {
    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;

    // Victim indexes a file whose name and content are both distinctive.
    let resp = f
        .client
        .upload_file(
            token,
            &f.repo_id,
            "/",
            "topsecretfile.txt",
            b"zqxjwv secret payload",
        )
        .await;
    assert_eq!(resp.status(), 200, "upload should succeed");

    // Wait until the victim can find it — proves the document is indexed.
    assert!(
        wait_for(std::time::Duration::from_secs(15), || async {
            !search_results(&f, token, "zqxjwv").await.is_empty()
        })
        .await,
        "victim should find their own indexed file"
    );

    // A second user with no repos at all must see nothing.
    let other = common::TestFixture::no_repo("nosy@example.com", "password").await;
    let other_token = &other.api_token;

    for (q, repo) in [
        ("zqxjwv", None),                                         // no search_repo at all
        ("topsecretfile", None),                                  // filename term
        ("zqxjwv", Some(f.repo_id.as_str())),                     // explicit victim repo id
        ("zqxjwv", Some("00000000-0000-0000-0000-000000000000")), // bogus id
    ] {
        let results = search_results_with_repo(&other, other_token, q, repo).await;
        assert!(
            results.is_empty(),
            "zero-repo user must not see other repos' files (q={q:?}, search_repo={repo:?}), got {results:?}"
        );
    }

    // Filename-only mode must not leak either.
    let resp = other
        .client
        .get(
            "/api2/search/?q=topsecretfile&search_filename_only=true",
            Some(other_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body["results"].as_array().unwrap().is_empty(),
        "filename-only search must not leak other repos' files"
    );
}

/// Control for a user with an accessible repo still finds their own
/// indexed content, so the fix does not disable search altogether.
#[tokio::test]
async fn test_owner_still_finds_own_content_after_search_fix() {
    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;

    let resp = f
        .client
        .upload_file(token, &f.repo_id, "/", "mine.txt", b"qqzzxx owner content")
        .await;
    assert_eq!(resp.status(), 200);

    assert!(
        wait_for(std::time::Duration::from_secs(15), || async {
            !search_results(&f, token, "qqzzxx").await.is_empty()
        })
        .await,
        "owner must still find their own indexed content"
    );

    // Scoping to the owner's own repo also works.
    let scoped = search_results_with_repo(&f, token, "qqzzxx", Some(f.repo_id.as_str())).await;
    assert!(
        !scoped.is_empty(),
        "owner must find content when scoping to their own repo"
    );
}

/// Compatibility: seahub lowercases `search_repo` before comparing it against
/// the scope keywords (`api2/views.py`: `search_repo = search_repo.lower()`),
/// so an uppercase scope must still mean "search all accessible repos" rather
/// than being treated as an inaccessible repo id.
#[tokio::test]
async fn test_uppercase_search_scope_is_accepted() {
    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;

    let resp = f
        .client
        .upload_file(token, &f.repo_id, "/", "scoped.txt", b"zzqquu scoped")
        .await;
    assert_eq!(resp.status(), 200);

    assert!(
        wait_for(std::time::Duration::from_secs(15), || async {
            !search_results(&f, token, "zzqquu").await.is_empty()
        })
        .await,
        "file should become searchable"
    );

    for scope in ["all", "ALL", "All", "MINE", "Shared"] {
        let results = search_results_with_repo(&f, token, "zzqquu", Some(scope)).await;
        assert!(
            !results.is_empty(),
            "scope {scope:?} must search the caller's accessible repos"
        );
    }

    // A repo id is still matched exactly (canonical lowercase form).
    let scoped = search_results_with_repo(&f, token, "zzqquu", Some(f.repo_id.as_str())).await;
    assert!(!scoped.is_empty(), "repo-id scoping must still work");
}

/// Absurd `page`/`per_page` values must be clamped rather than overflow.
///
/// `(page - 1) * per_page` was computed in `i32`, so a client could panic the
/// handler in debug/test builds (overflow check) or get a wrapped, meaningless
/// offset in release builds.
#[tokio::test]
async fn test_search_pagination_is_clamped() {
    let f = common::TestFixture::new_with_index().await;

    for query in [
        "?q=no-such-file-anywhere&page=1000000&per_page=10000000",
        "?q=no-such-file-anywhere&page=2147483647&per_page=2147483647",
        "?q=no-such-file-anywhere&page=0&per_page=0",
    ] {
        let resp = f
            .client
            .get(&format!("/api2/search/{query}"), Some(&f.api_token))
            .await;
        assert_eq!(resp.status(), 200, "query {query} should not fail");
        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(
            body["results"].as_array().unwrap().is_empty(),
            "no results expected for {query}"
        );
    }

    // A normal page still returns the normal envelope.
    let body: serde_json::Value = f
        .client
        .get(
            "/api2/search/?q=no-such-file-anywhere&page=1&per_page=10",
            Some(&f.api_token),
        )
        .await
        .json()
        .await
        .unwrap();
    assert!(body["results"].is_array());
}

/// The low-load backfill is what indexes files the pipeline never saw: files
/// that predate it, and files an extractor gained support for since.
///
/// The document is removed by hand to stand in for "this file was uploaded
/// before the pipeline existed"; the pass must find it from the repository tree
/// alone, and a second pass must then find nothing left to do.
#[tokio::test]
async fn a_backfill_pass_indexes_a_file_the_index_never_saw() {
    use server::service::index::IndexService;
    use server::tasks::spec::JobKey;

    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;
    let path = "/legacy-note.txt";

    let resp = f
        .client
        .upload_file(
            token,
            &f.repo_id,
            "/",
            "legacy-note.txt",
            b"backfill unique needle",
        )
        .await;
    assert_eq!(resp.status(), 200, "upload should succeed");

    // Wait until the scheduled index run has both written the document and
    // finished, so removing it below is not racing a run in flight.
    assert!(
        wait_for(std::time::Duration::from_secs(15), || async {
            !search_results(&f, token, "needle").await.is_empty()
                && f.server
                    .state
                    .tasks
                    .store()
                    .count_active(JobKey::IndexFiles, None)
                    == 0
        })
        .await,
        "the upload should have been indexed"
    );

    let indexer = f.server.state.indexer.clone().expect("indexer is enabled");
    indexer
        .delete_file_async(&f.repo_id, path)
        .await
        .expect("delete the document");
    indexer.commit().expect("commit the delete");
    assert!(
        search_results(&f, token, "needle").await.is_empty(),
        "the file is invisible to search until it is indexed again"
    );

    let svc = IndexService::new(
        f.server.state.repos.clone(),
        f.server.state.block_store.clone(),
        indexer.clone(),
    );
    let cursor = std::sync::Arc::new(std::sync::Mutex::new(0usize));
    let report = svc.backfill_pass(None, &cursor).await.expect("pass runs");
    assert!(
        report.counts.indexed >= 1,
        "the pass should have indexed the file: {report:?}"
    );
    indexer.commit().expect("commit");

    assert!(
        wait_for(std::time::Duration::from_secs(15), || async {
            !search_results(&f, token, "needle").await.is_empty()
        })
        .await,
        "the backfilled file should be searchable"
    );

    // The state the pass wrote says the file is done, so a second pass is a
    // no-op rather than a re-read.
    let report = svc.backfill_pass(None, &cursor).await.expect("pass runs");
    assert_eq!(
        report.counts.indexed, 0,
        "nothing is left to index: {report:?}"
    );
}

/// Ask the server to index one path synchronously and report whether it
/// became searchable.
async fn reindex_path(f: &common::TestFixture, token: &str, path: &str) -> bool {
    let resp = f
        .client
        .post_json(
            &format!("/api2/repos/{}/file/reindex/", f.repo_id),
            Some(token),
            &serde_json::json!({ "p": path }),
        )
        .await;
    assert_eq!(resp.status(), 200, "reindex {path} should succeed");
    let body: serde_json::Value = resp.json().await.unwrap();
    body["indexed"].as_bool().expect("indexed flag")
}

/// Every document format the extractor claims is parsed and becomes
/// searchable — the PDF from its text object, the Office files from their
/// package parts.
#[tokio::test]
async fn document_formats_are_searchable() {
    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;

    let documents: [(&str, Vec<u8>, &str); 4] = [
        (
            "report.pdf",
            common::minimal_pdf("zebraquartz"),
            "zebraquartz",
        ),
        (
            "notes.docx",
            common::minimal_docx("wombatpuzzle"),
            "wombatpuzzle",
        ),
        (
            "book.xlsx",
            common::minimal_xlsx("xylophonebudget"),
            "xylophonebudget",
        ),
        (
            "talk.pptx",
            common::minimal_pptx("narwhaloverview"),
            "narwhaloverview",
        ),
    ];

    for (name, data, _) in &documents {
        let resp = f
            .client
            .upload_file(token, &f.repo_id, "/", name, data)
            .await;
        assert_eq!(resp.status(), 200, "upload {name} should succeed");
    }

    for (name, _, keyword) in &documents {
        assert!(
            wait_for(std::time::Duration::from_secs(60), || async {
                !search_results(&f, token, keyword).await.is_empty()
            })
            .await,
            "{name} content should be searchable"
        );
    }
}

/// Files the extractor cannot turn into text are recorded as skipped, and
/// neither the request nor the batch that carried them fails.
///
/// Three shapes: a damaged container the lenient reader still opens, input that
/// is not a PDF at all, and an Office package that claims to expand far past
/// the extraction budget. The last one is the shape a crafted file takes, and
/// it is exercised through the whole pipeline rather than only in a unit test.
#[tokio::test]
async fn an_unreadable_document_is_skipped_without_breaking_the_batch() {
    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;

    let bomb = common::bomb_docx();
    let unreadable: [(&str, &[u8]); 3] = [
        ("corrupt.pdf", include_bytes!("fixtures/corrupt.pdf")),
        (
            "garbage.pdf",
            b"this file is not a PDF, whatever its name says" as &[u8],
        ),
        ("bomb.docx", bomb.as_slice()),
    ];

    for (name, data) in unreadable {
        let resp = f
            .client
            .upload_file(token, &f.repo_id, "/", name, data)
            .await;
        assert_eq!(resp.status(), 200, "upload {name} should succeed");
    }

    for (name, _) in unreadable {
        let path = format!("/{name}");
        assert!(
            !reindex_path(&f, token, &path).await,
            "{name} must not report as indexed"
        );
    }

    // The server is still working: a file uploaded afterwards still reaches
    // the index.
    let resp = f
        .client
        .upload_file(token, &f.repo_id, "/", "after.txt", b"aftermath signal")
        .await;
    assert_eq!(resp.status(), 200);
    assert!(
        wait_for(std::time::Duration::from_secs(30), || async {
            !search_results(&f, token, "aftermath").await.is_empty()
        })
        .await,
        "an unreadable document must not stop later files from being indexed"
    );
}

/// A PDF with no text layer parses fine and yields nothing. That is a skip,
/// not an indexed document with empty content — the file stays findable by
/// name, which is all it has.
#[tokio::test]
async fn a_document_without_a_text_layer_is_not_indexed() {
    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;

    let resp = f
        .client
        .upload_file(
            token,
            &f.repo_id,
            "/",
            "blank.pdf",
            &common::minimal_pdf(""),
        )
        .await;
    assert_eq!(resp.status(), 200);

    assert!(
        !reindex_path(&f, token, "/blank.pdf").await,
        "a document with no extractable text must not report as indexed"
    );

    // Still reachable by filename, so the skip costs the file nothing but its
    // (nonexistent) content.
    assert!(
        wait_for(std::time::Duration::from_secs(15), || async {
            let results = search_results(&f, token, "blank").await;
            results
                .iter()
                .any(|r| r["name"].as_str().is_some_and(|n| n == "blank.pdf"))
        })
        .await,
        "an unindexed document should still be findable by filename"
    );
}

/// A document larger than the cap is skipped rather than read: the pipeline
/// must not spend a block read on a file it cannot parse from a prefix.
#[tokio::test]
async fn an_oversized_document_is_skipped() {
    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;

    // A valid PDF header followed by filler, larger than the structured cap.
    let mut oversized = common::minimal_pdf("zebraquartz");
    oversized.resize(server::indexer::extract::MAX_STRUCTURED_BYTES + 1, 0x20);

    let resp = f
        .client
        .upload_file(token, &f.repo_id, "/", "huge.pdf", &oversized)
        .await;
    assert_eq!(resp.status(), 200, "upload should succeed");

    assert!(
        !reindex_path(&f, token, "/huge.pdf").await,
        "an oversized document must not report as indexed"
    );
}

/// The headline of the upgrade: a file the *previous* extractor examined and
/// skipped becomes searchable on the next quiet pass, with no manual rebuild.
///
/// The file content is unchanged and the recorded `fs_id` still matches, so the
/// only thing marking it stale is the extractor version — which is exactly the
/// mechanism that picks up libraries indexed before PDF support existed.
#[tokio::test]
async fn a_backfill_pass_picks_up_what_an_older_extractor_skipped() {
    use server::indexer::extract::EXTRACTOR_VERSION;
    use server::indexer::{DocMeta, DocStatus};
    use server::service::index::IndexService;
    use server::tasks::spec::JobKey;
    use std::sync::{Arc, Mutex};

    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;
    let path = "/handbook.pdf";

    let resp = f
        .client
        .upload_file(
            token,
            &f.repo_id,
            "/",
            "handbook.pdf",
            &common::minimal_pdf("upgradeneedle"),
        )
        .await;
    assert_eq!(resp.status(), 200, "upload should succeed");

    assert!(
        wait_for(std::time::Duration::from_secs(30), || async {
            !search_results(&f, token, "upgradeneedle").await.is_empty()
                && f.server
                    .state
                    .tasks
                    .store()
                    .count_active(JobKey::IndexFiles, None)
                    == 0
        })
        .await,
        "the document should be indexed to begin with"
    );

    let indexer = f.server.state.indexer.clone().expect("indexer is enabled");

    // Keep the content identity, so only the version can make this stale.
    let fs_id = indexer
        .doc_states(&f.repo_id, &[path.to_string()])
        .expect("read document states")
        .get(path)
        .expect("the document was just indexed")
        .fs_id
        .clone();
    assert!(!fs_id.is_empty(), "the document records its content id");

    indexer
        .mark_file_async(
            &f.repo_id,
            path,
            "handbook.pdf",
            DocMeta {
                fs_id: fs_id.clone(),
                extractor_version: EXTRACTOR_VERSION - 1,
                status: DocStatus::Skipped,
                attempted_at: 0,
            },
        )
        .await
        .expect("rewrite the document as the older extractor left it");
    indexer.commit().expect("commit");

    let svc = IndexService::new(
        f.server.state.repos.clone(),
        f.server.state.block_store.clone(),
        indexer.clone(),
    );
    let cursor = Arc::new(Mutex::new(0));
    let report = svc.backfill_pass(None, &cursor).await.expect("pass runs");
    assert!(
        report.counts.indexed >= 1,
        "the version bump should make the pass re-read the file: {report:?}"
    );
    indexer.commit().expect("commit");

    assert!(
        wait_for(std::time::Duration::from_secs(15), || async {
            !search_results(&f, token, "upgradeneedle").await.is_empty()
        })
        .await,
        "the document should be searchable again after the pass"
    );

    // Now that it is current, a second pass leaves it alone.
    let report = svc.backfill_pass(None, &cursor).await.expect("pass runs");
    assert_eq!(
        report.counts.indexed, 0,
        "the file is current again: {report:?}"
    );
}
