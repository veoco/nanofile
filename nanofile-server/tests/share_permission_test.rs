mod common;

use common::TestFixture;

/// C.3.1 — GET /api2/beshared-repos/{repo_id}/ — list share members.
#[tokio::test]
async fn test_list_share_members_empty() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .get(
            &format!("/api2/beshared-repos/{}/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: Vec<serde_json::Value> = resp.json().await.unwrap();
    // The repo owner is always a member.
    assert_eq!(body.len(), 1);
    assert_eq!(body[0]["email"], "test@example.com");
    assert_eq!(body[0]["permission"], "rw");
}

/// C.3.2 — POST /api2/beshared-repos/{repo_id}/ — share repo with another user.
#[tokio::test]
async fn test_share_repo_with_user() {
    let f = TestFixture::new().await;

    // Create a second user to share with.
    let _user2_id = common::create_test_user(&f.server.db, "user2@test.com", "password").await;

    let resp = f
        .client
        .post_json(
            &format!("/api2/beshared-repos/{}/", f.repo_id),
            Some(&f.api_token),
            &serde_json::json!({
                "share_type": "personal",
                "user": "user2@test.com",
                "permission": "rw"
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);
}

/// C.3.3 — PUT /api2/beshared-repos/{repo_id}/ — modify share permission.
#[tokio::test]
async fn test_modify_share_permission() {
    let f = TestFixture::new().await;

    // Create a second user and share the repo.
    let _user2_id = common::create_test_user(&f.server.db, "modify@test.com", "password").await;

    let resp = f
        .client
        .post_json(
            &format!("/api2/beshared-repos/{}/", f.repo_id),
            Some(&f.api_token),
            &serde_json::json!({
                "share_type": "personal",
                "user": "modify@test.com",
                "permission": "rw"
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);

    // Modify permission to read-only.
    let resp = f
        .client
        .put_json(
            &format!("/api2/beshared-repos/{}/", f.repo_id),
            Some(&f.api_token),
            &serde_json::json!({
                "user": "modify@test.com",
                "permission": "r"
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["success"], true);

    // Verify via GET.
    let resp = f
        .client
        .get(
            &format!("/api2/beshared-repos/{}/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let members: Vec<serde_json::Value> = resp.json().await.unwrap();
    let mod_member = members
        .iter()
        .find(|m| m["email"] == "modify@test.com")
        .unwrap();
    assert_eq!(mod_member["permission"], "r");
}

/// C.3.4 — DELETE /api2/beshared-repos/{repo_id}/ — remove share.
#[tokio::test]
async fn test_delete_share() {
    let f = TestFixture::new().await;

    // Create a second user and share the repo.
    let _user2_id = common::create_test_user(&f.server.db, "delete@test.com", "password").await;

    let resp = f
        .client
        .post_json(
            &format!("/api2/beshared-repos/{}/", f.repo_id),
            Some(&f.api_token),
            &serde_json::json!({
                "share_type": "personal",
                "user": "delete@test.com",
                "permission": "rw"
            }),
        )
        .await;
    assert_eq!(resp.status(), 200);

    // Delete the share.
    let http_client = reqwest::Client::new();
    let resp = http_client
        .delete(format!(
            "{}/api2/beshared-repos/{}/",
            f.server.base_url, f.repo_id
        ))
        .bearer_auth(&f.api_token)
        .json(&serde_json::json!({"user": "delete@test.com"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Verify via GET — should be empty.
    let resp = f
        .client
        .get(
            &format!("/api2/beshared-repos/{}/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let members: Vec<serde_json::Value> = resp.json().await.unwrap();
    assert!(
        !members.iter().any(|m| m["email"] == "delete@test.com"),
        "deleted user should not appear in share list"
    );
}

/// C.3.5 — Authorization: non-owner cannot modify permissions.
#[tokio::test]
async fn test_modify_share_unauthorized() {
    let f = TestFixture::new().await;

    // Create another user who doesn't own the repo.
    let _user2_id = common::create_test_user(&f.server.db, "other@test.com", "password").await;

    let resp = f.client.login("other@test.com", "password").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let other_token = body["token"].as_str().unwrap();

    let resp = f
        .client
        .put_json(
            &format!("/api2/beshared-repos/{}/", f.repo_id),
            Some(other_token),
            &serde_json::json!({
                "user": "other@test.com",
                "permission": "rw"
            }),
        )
        .await;
    assert_eq!(resp.status(), 403);
}
