mod common;

use common::TestFixture;
use common::create_test_user;
use serde_json::Value;

// ── Helpers ──────────────────────────────────────────────────────────────

/// Upload a small file to a repo.
async fn upload_file(f: &TestFixture, name: &str) {
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", name, b"content")
        .await;
    assert_eq!(resp.status(), 200, "upload failed: {:?}", resp.text().await);
}

/// Create a subdirectory.
async fn create_dir(f: &TestFixture, path: &str) {
    let resp = f.client.create_dir(&f.api_token, &f.repo_id, path).await;
    assert_eq!(resp.status(), 200, "mkdir failed: {:?}", resp.text().await);
}

/// Create empty file via v2.1.
async fn create_file(f: &TestFixture, path: &str) {
    let resp = f
        .client
        .post_json(
            &format!("/api/v2.1/repos/{}/file/", f.repo_id),
            Some(&f.api_token),
            &serde_json::json!({"p": path}),
        )
        .await;
    assert_eq!(
        resp.status(),
        200,
        "create_file failed: {:?}",
        resp.text().await
    );
}

/// Fetch activities and return the events array + total_count.
async fn get_activities(f: &TestFixture, page: u32, per_page: u32) -> (Vec<Value>, usize) {
    let resp = f
        .client
        .get(
            &format!("/api/v2.1/activities/?page={}&per_page={}", page, per_page),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let events = body["events"].as_array().unwrap().clone();
    let total = body["total_count"].as_u64().unwrap_or(0) as usize;
    (events, total)
}

/// Find an event by op_type. Returns the first match.
fn find_event<'a>(events: &'a [Value], op_type: &str) -> Option<&'a Value> {
    events.iter().find(|ev| ev["op_type"] == op_type)
}

/// Helper: create second user and return its api_token.
async fn create_second_user(f: &TestFixture) -> String {
    let db = &*f.server.db;
    let _uid = create_test_user(db, "user2@test.com", "password2").await;
    let resp = f.client.login("user2@test.com", "password2").await;
    assert_eq!(resp.status(), 200);
    let tv: Value = resp.json().await.unwrap();
    tv["token"].as_str().unwrap().to_string()
}

// ── Tests ────────────────────────────────────────────────────────────────

/// Initial fixture creation produces exactly one event: repo creation.
#[tokio::test]
async fn test_activities_initial_state() {
    let f = TestFixture::new().await;
    let (events, total) = get_activities(&f, 1, 10).await;
    assert_eq!(total, 1, "expected 1 event (repo create), got {total}");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["op_type"], "create");
    assert_eq!(events[0]["obj_type"], "repo");
}

#[tokio::test]
async fn test_activity_after_file_create() {
    let f = TestFixture::new().await;
    create_file(&f, "/created.txt").await;

    let (events, _) = get_activities(&f, 1, 10).await;
    assert_eq!(events.len(), 2, "expected 2 events (repo + file create)");
    // Events ordered by created_at DESC, so events[0] is most recent (file create)
    let ev = &events[0];
    assert_eq!(ev["op_type"], "create");
    assert_eq!(ev["obj_type"], "file");
    assert_eq!(ev["name"], "created.txt");
    assert_eq!(ev["author_email"], f.email);
    assert!(
        ev["time"].as_str().unwrap_or("").contains('T'),
        "time should be ISO 8601"
    );
    assert!(
        !ev["commit_id"].as_str().unwrap_or("").is_empty(),
        "commit_id should not be empty"
    );
}

#[tokio::test]
async fn test_activity_after_file_upload() {
    let f = TestFixture::new().await;
    upload_file(&f, "uploaded.txt").await;

    let (events, _) = get_activities(&f, 1, 10).await;
    assert_eq!(
        events.len(),
        2,
        "expected 2 events (repo + upload), got: {events:?}"
    );
}

#[tokio::test]
async fn test_activity_after_file_rename() {
    let f = TestFixture::new().await;
    create_file(&f, "/old_name.txt").await;

    // Rename via v2 JSON endpoint
    let resp = f
        .client
        .rename_file(&f.api_token, &f.repo_id, "/old_name.txt", "new_name.txt")
        .await;
    assert_eq!(resp.status(), 200, "rename failed: {:?}", resp.text().await);

    let (events, _) = get_activities(&f, 1, 10).await;
    assert_eq!(
        events.len(),
        3,
        "expected 3 events (repo + create + rename), got: {events:?}"
    );

    // Verify rename event has old_path
    let rename_ev = find_event(&events, "rename").expect("rename event should exist");
    assert_eq!(rename_ev["path"], "/new_name.txt");
    assert_eq!(rename_ev["old_path"], "/old_name.txt");
    assert_eq!(rename_ev["name"], "new_name.txt");
}

#[tokio::test]
async fn test_activity_after_file_move() {
    let f = TestFixture::new().await;
    create_dir(&f, "/dest").await;
    create_file(&f, "/moveme.txt").await;

    // Move via v2 JSON endpoint
    let resp = f
        .client
        .move_file(&f.api_token, &f.repo_id, "/moveme.txt", &f.repo_id, "/dest")
        .await;
    assert_eq!(resp.status(), 200, "move failed: {:?}", resp.text().await);

    let (events, _) = get_activities(&f, 1, 10).await;
    assert_eq!(
        events.len(),
        4,
        "expected 4 events (repo + mkdir + create + move), got: {events:?}"
    );

    let move_ev = find_event(&events, "move").expect("move event should exist");
    assert_eq!(move_ev["path"], "/dest/moveme.txt");
    assert_eq!(move_ev["old_path"], "/moveme.txt");
}

#[tokio::test]
async fn test_activity_after_file_delete() {
    let f = TestFixture::new().await;
    create_file(&f, "/deleteme.txt").await;

    // Delete via v2.1
    let resp = f
        .client
        .delete(
            &format!("/api/v2.1/repos/{}/file/?p=/deleteme.txt", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200, "delete failed: {:?}", resp.text().await);

    let (events, _) = get_activities(&f, 1, 10).await;
    assert_eq!(
        events.len(),
        3,
        "expected 3 events (repo + create + delete)"
    );

    let delete_ev = find_event(&events, "delete").expect("delete event should exist");
    assert_eq!(delete_ev["obj_type"], "file");
    assert_eq!(delete_ev["path"], "/deleteme.txt");
}

#[tokio::test]
async fn test_activity_after_dir_create() {
    let f = TestFixture::new().await;
    create_dir(&f, "/newfolder").await;

    let (events, _) = get_activities(&f, 1, 10).await;
    assert_eq!(
        events.len(),
        2,
        "expected 2 events (repo + mkdir), got: {events:?}"
    );
    // Events ordered by created_at DESC, so events[0] is most recent (mkdir)
    assert_eq!(events[0]["op_type"], "create");
    assert_eq!(events[0]["obj_type"], "dir");
}

#[tokio::test]
async fn test_activity_after_dir_delete() {
    let f = TestFixture::new().await;
    create_dir(&f, "/killdir").await;

    // Delete dir via v2.1
    let resp = f
        .client
        .delete(
            &format!("/api/v2.1/repos/{}/dir/?p=/killdir", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(
        resp.status(),
        200,
        "delete dir failed: {:?}",
        resp.text().await
    );

    let (events, _) = get_activities(&f, 1, 10).await;
    assert_eq!(events.len(), 3, "expected 3 events (repo + mkdir + delete)");

    let delete_ev = find_event(&events, "delete").expect("delete event should exist");
    assert_eq!(delete_ev["obj_type"], "dir");
}

#[tokio::test]
async fn test_activity_after_dir_rename() {
    let f = TestFixture::new().await;
    create_dir(&f, "/olddirname").await;

    // Rename dir via JSON endpoint
    let resp = f
        .client
        .post_json(
            &format!("/api2/repos/{}/dir/rename/", f.repo_id),
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "p": "/olddirname",
                "new_name": "newdirname"
            }),
        )
        .await;
    assert_eq!(
        resp.status(),
        200,
        "rename dir failed: {:?}",
        resp.text().await
    );

    let (events, _) = get_activities(&f, 1, 10).await;
    assert_eq!(events.len(), 3, "expected 3 events (repo + mkdir + rename)");

    let rename_ev = find_event(&events, "rename").expect("rename event should exist");
    assert_eq!(rename_ev["obj_type"], "dir");
    assert_eq!(rename_ev["path"], "/newdirname");
    assert_eq!(rename_ev["old_path"], "/olddirname");
}

#[tokio::test]
async fn test_activity_after_dir_move() {
    let f = TestFixture::new().await;
    create_dir(&f, "/sourcedir").await;
    create_dir(&f, "/target").await;

    // Move dir via JSON endpoint
    let resp = f
        .client
        .post_json(
            &format!("/api2/repos/{}/dir/move/", f.repo_id),
            Some(&f.api_token),
            &serde_json::json!({
                "repo_id": f.repo_id,
                "p": "/sourcedir",
                "new_parent_dir": "/target"
            }),
        )
        .await;
    assert_eq!(
        resp.status(),
        200,
        "move dir failed: {:?}",
        resp.text().await
    );

    let (events, _) = get_activities(&f, 1, 10).await;
    assert_eq!(events.len(), 4, "expected 4 events (repo + 2 mkdir + move)");

    let move_ev = find_event(&events, "move").expect("move event should exist");
    assert_eq!(move_ev["obj_type"], "dir");
    assert_eq!(move_ev["path"], "/target/sourcedir");
    assert_eq!(move_ev["old_path"], "/sourcedir");
}

#[tokio::test]
async fn test_activity_pagination() {
    let f = TestFixture::new().await;

    // Create 5 files. Order within the same second is undefined,
    // but pagination should still work correctly.
    for i in 1..=5 {
        create_file(&f, &format!("/file{i}.txt")).await;
    }

    // page=1, per_page=2 → 2 events, total=6 (repo + 5 files)
    let (events, total) = get_activities(&f, 1, 2).await;
    assert_eq!(events.len(), 2, "expected 2 events on page 1");
    assert_eq!(total, 6);

    // page=3, per_page=2 → 2 events (last page: repo + 1 file)
    let (events_page3, _) = get_activities(&f, 3, 2).await;
    assert_eq!(events_page3.len(), 2, "expected 2 events on page 3");
}

#[tokio::test]
async fn test_activity_repo_id_filter() {
    let f = TestFixture::new().await;

    // Create activity in the default repo
    create_file(&f, "/repo1-file.txt").await;

    // Create a second repo and create activity there
    let resp = f.client.create_repo(&f.api_token, "second-repo").await;
    assert!(
        resp.status().is_success(),
        "create repo failed: {:?}",
        resp.text().await
    );
    let body: Value = resp.json().await.unwrap();
    let repo2_id = body["repo_id"].as_str().unwrap_or("").to_string();

    // Create file in repo2
    let resp = f
        .client
        .post_json(
            &format!("/api/v2.1/repos/{}/file/", repo2_id),
            Some(&f.api_token),
            &serde_json::json!({"p": "/repo2-file.txt"}),
        )
        .await;
    assert_eq!(resp.status(), 200);

    // Filter by repo2 — should see 2 events (repo2 create + file create in repo2)
    let resp = f
        .client
        .get(
            &format!("/api/v2.1/activities/?repo_id={}", repo2_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let events = body["events"].as_array().unwrap();
    assert_eq!(
        events.len(),
        2,
        "expected 2 events for repo2 (repo create + file create)"
    );
    assert_eq!(events[0]["repo_id"], repo2_id);
}

#[tokio::test]
async fn test_activity_unauthorized() {
    let f = TestFixture::new().await;
    let resp = f.client.get("/api/v2.1/activities/", None).await;
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_activity_response_fields() {
    let f = TestFixture::new().await;
    create_file(&f, "/fields-test.txt").await;

    let (events, _) = get_activities(&f, 1, 10).await;
    // Find the file create event (skip repo create)
    let ev = find_event(&events, "create").expect("create event should exist");
    assert_eq!(ev["obj_type"], "file");

    // Verify all expected response fields
    assert_eq!(ev["op_type"], "create");
    assert_eq!(ev["repo_id"], f.repo_id);
    assert!(!ev["repo_name"].as_str().unwrap_or("").is_empty());
    assert!(!ev["commit_id"].as_str().unwrap_or("").is_empty());
    assert_eq!(ev["path"], "/fields-test.txt");
    assert_eq!(ev["name"], "fields-test.txt");
    assert_eq!(ev["author_email"], f.email);
    assert_eq!(ev["author_name"], f.email.split('@').next().unwrap_or(""));
    assert_eq!(ev["author_contact_email"], f.email);
    assert!(ev["time"].as_str().unwrap_or("").contains('T'));
    assert_eq!(ev["details"], serde_json::json!([]));
    assert_eq!(ev["count"], 0);
}

#[tokio::test]
async fn test_activity_old_path_in_response() {
    let f = TestFixture::new().await;
    create_file(&f, "/original.txt").await;

    // Rename
    let resp = f
        .client
        .rename_file(&f.api_token, &f.repo_id, "/original.txt", "renamed.txt")
        .await;
    assert_eq!(resp.status(), 200);

    let (events, _) = get_activities(&f, 1, 10).await;

    let rename_ev = find_event(&events, "rename").expect("rename event should exist");
    assert_eq!(rename_ev["op_type"], "rename");
    assert!(
        rename_ev.get("old_path").is_some(),
        "rename event should have old_path"
    );
    assert_eq!(rename_ev["old_path"], "/original.txt");
}

#[tokio::test]
async fn test_activity_multiple_operations() {
    let f = TestFixture::new().await;
    let name = "multi.txt";

    // 1. Create
    create_file(&f, &format!("/{name}")).await;

    // 2. Rename
    let _ = f
        .client
        .rename_file(
            &f.api_token,
            &f.repo_id,
            &format!("/{name}"),
            "multi_v2.txt",
        )
        .await;

    // 3. Delete
    let _ = f
        .client
        .delete(
            &format!("/api/v2.1/repos/{}/file/?p=/multi_v2.txt", f.repo_id),
            Some(&f.api_token),
        )
        .await;

    let (events, total) = get_activities(&f, 1, 10).await;
    assert_eq!(
        total, 4,
        "expected 4 total events (repo + create + rename + delete)"
    );
    // Verify each operation type appears
    assert!(
        find_event(&events, "create").is_some(),
        "expected a create event"
    );
    assert!(
        find_event(&events, "rename").is_some(),
        "expected a rename event"
    );
    assert!(
        find_event(&events, "delete").is_some(),
        "expected a delete event"
    );
}

#[tokio::test]
async fn test_activity_op_user_filter() {
    let f = TestFixture::new().await;

    // Create a file
    create_file(&f, "/opuser-test.txt").await;

    // Filter by own email — should see activities (repo create + file create)
    let resp = f
        .client
        .get(
            &format!("/api/v2.1/activities/?op_user={}", f.email),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let events = body["events"].as_array().unwrap();
    assert!(
        events.len() >= 2,
        "filtering by own email should show own events, got {}",
        events.len()
    );
}

#[tokio::test]
async fn test_activity_op_user_nonexistent() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .get(
            "/api/v2.1/activities/?op_user=nobody@nowhere.com",
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let events = body["events"].as_array().unwrap();
    assert_eq!(events.len(), 0, "nonexistent user should have 0 events");
    assert_eq!(body["total_count"], 0);
}

#[tokio::test]
async fn test_activity_cross_user_visibility() {
    let f = TestFixture::new().await;

    // Create a second user
    let api_token2 = create_second_user(&f).await;

    // Share the repo with user2 via beshare API
    let share_resp = f
        .client
        .post_json(
            &format!("/api2/beshared-repos/{}/", f.repo_id),
            Some(&f.api_token),
            &serde_json::json!({
                "share_type": "user",
                "user": "user2@test.com",
                "permission": "rw",
            }),
        )
        .await;
    assert_eq!(share_resp.status(), 200, "share repo with user2 failed");

    // User1 creates a file in the shared repo
    create_file(&f, "/shared-file.txt").await;

    // User2 should be able to see User1's activity because they share the repo
    let resp = f
        .client
        .get("/api/v2.1/activities/", Some(&api_token2))
        .await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let events = body["events"].as_array().unwrap();
    assert!(
        !events.is_empty(),
        "user2 should see activities in shared repo"
    );

    // The file create event should have user1's email
    let file_create = events
        .iter()
        .find(|e| e["op_type"] == "create" && e["obj_type"] == "file");
    assert!(
        file_create.is_some(),
        "user2 should see user1's file create event"
    );
}

#[tokio::test]
async fn test_activity_repo_created_logged() {
    let f = TestFixture::new().await;
    let (events, total) = get_activities(&f, 1, 10).await;
    assert!(total >= 1, "should have at least one event");

    let create_ev = find_event(&events, "create").expect("create event should exist");
    assert_eq!(create_ev["obj_type"], "repo");
    assert_eq!(create_ev["repo_id"], f.repo_id);
    assert!(!create_ev["repo_name"].as_str().unwrap_or("").is_empty());
}

#[tokio::test]
async fn test_activity_repo_deleted_logged() {
    let f = TestFixture::new().await;
    let repo_id1 = f.repo_id.clone();

    // Create a second repo to verify its activities survive FK cascade
    let resp = f.client.create_repo(&f.api_token, "survivor-repo").await;
    assert!(resp.status().is_success());
    let body: Value = resp.json().await.unwrap();
    let repo_id2 = body["repo_id"].as_str().unwrap().to_string();

    // List activities before deletion — should have 2 repo-create events
    let (_events_before, total_before) = get_activities(&f, 1, 10).await;
    assert_eq!(
        total_before, 2,
        "should have 2 repo create events before delete"
    );

    // Delete the first repo (activities cascade-deleted via FK)
    let resp = f
        .client
        .delete(&format!("/api2/repos/{}/", repo_id1), Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200, "delete repo failed");

    // After deletion, repo1's activities are cascade-deleted,
    // but repo2's create activity should remain
    let (events_after, total_after) = get_activities(&f, 1, 10).await;
    assert_eq!(
        total_after, 1,
        "only survivor repo's create activity should remain"
    );
    assert_eq!(events_after[0]["repo_id"], repo_id2);
    assert_eq!(events_after[0]["op_type"], "create");
    assert_eq!(events_after[0]["obj_type"], "repo");
}

#[tokio::test]
async fn test_ui_activities_page() {
    let f = TestFixture::new().await;

    // Use a single cookie client for login and page request
    let ui_client = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();
    let login_resp = ui_client
        .post(format!("{}/accounts/login/", f.server.base_url))
        .form(&[("email", f.email.as_str()), ("password", "password")])
        .send()
        .await
        .unwrap();
    assert!(
        login_resp.status() == 302 || login_resp.status() == 200,
        "login failed with status {}: {:?}",
        login_resp.status(),
        login_resp.text().await
    );

    let resp = ui_client
        .get(format!("{}/activities/", f.server.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let html = resp.text().await.unwrap();
    assert!(
        html.contains("Activities"),
        "page should contain 'Activities', got first 300 chars: {:?}",
        &html[..html.len().min(300)]
    );
}
