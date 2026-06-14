mod common;

use common::TestFixture;
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

// ── Tests ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_activities_empty() {
    let f = TestFixture::new().await;
    let (events, total) = get_activities(&f, 1, 10).await;
    assert!(
        events.is_empty(),
        "expected empty events, got {} items",
        events.len()
    );
    assert_eq!(total, 0);
}

#[tokio::test]
async fn test_activity_after_file_create() {
    let f = TestFixture::new().await;
    create_file(&f, "/created.txt").await;

    let (events, _) = get_activities(&f, 1, 10).await;
    assert_eq!(events.len(), 1);
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
        1,
        "expected 1 activity (upload), got: {events:?}"
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
        2,
        "expected 2 events (create + rename), got: {events:?}"
    );

    // Search for the rename event (order is undefined when timestamps match)
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
        3,
        "expected 3 events (mkdir + create + move), got: {events:?}"
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
    assert_eq!(events.len(), 2, "expected 2 events (create + delete)");

    let delete_ev = find_event(&events, "delete").expect("delete event should exist");
    assert_eq!(delete_ev["obj_type"], "file");
    assert_eq!(delete_ev["path"], "/deleteme.txt");
}

#[tokio::test]
async fn test_activity_after_dir_create() {
    let f = TestFixture::new().await;
    create_dir(&f, "/newfolder").await;

    let (events, _) = get_activities(&f, 1, 10).await;
    assert_eq!(events.len(), 1, "expected 1 event, got: {events:?}");
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
    assert_eq!(events.len(), 2, "expected 2 events (create + delete)");

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
    assert_eq!(events.len(), 2, "expected 2 events (mkdir + rename)");

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
    assert_eq!(events.len(), 3, "expected 3 events (2 mkdir + move)");

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

    // page=1, per_page=2 → 2 events, total=5
    let (events, total) = get_activities(&f, 1, 2).await;
    assert_eq!(events.len(), 2, "expected 2 events on page 1");
    assert_eq!(total, 5);

    // page=3, per_page=2 → 1 event (last page)
    let (events_page3, _) = get_activities(&f, 3, 2).await;
    assert_eq!(events_page3.len(), 1, "expected 1 event on page 3");
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

    // Filter by repo2
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
    assert_eq!(events.len(), 1, "expected 1 event for repo2");
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
    assert_eq!(events.len(), 1);
    let ev = &events[0];

    // Verify all expected response fields
    assert_eq!(ev["op_type"], "create");
    assert_eq!(ev["obj_type"], "file");
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
        total, 3,
        "expected 3 total events (create + rename + delete)"
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
