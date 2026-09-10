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
    assert_eq!(resp.status(), 200);

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

// ── Access control (C-2) ─────────────────────────────────────────────────────

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

/// Regression (C-2): a user with no accessible repos must not be able to use
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
            "C-2: zero-repo user must not see other repos' files (q={q:?}, search_repo={repo:?}), got {results:?}"
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
        "C-2: filename-only search must not leak other repos' files"
    );
}

/// Control for C-2: a user with an accessible repo still finds their own
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
