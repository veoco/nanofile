mod common;

use common::{TestFixture, create_test_user};

/// `GET /api/v2.1/repos/{repo_id}/related-users/` must return a `user_list` of
/// `{email, name, contact_email, avatar_url}` objects (seahub's
/// `get_user_common_info` shape), not the legacy `{"users": ["<user_id>"]}`.
/// The Android client's `UserWrapperModel.user_list` NPEs when the field is
/// missing or holds strings.
#[tokio::test]
async fn test_related_users_returns_user_list_objects() {
    let f = TestFixture::new().await;

    // Add a second user as a repo member.
    let user2_id = create_test_user(f.server.db.as_ref(), "second@example.com", "password").await;
    let now = chrono::Utc::now().timestamp();
    f.server
        .repos
        .member
        .create_member(server::repository::member::CreateMemberParams {
            repo_id: f.repo_id.clone(),
            user_id: user2_id,
            permission: "rw".to_string(),
            created_at: now,
        })
        .await
        .unwrap();

    let resp = f
        .client
        .get(
            &format!("/api/v2.1/repos/{}/related-users/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let user_list = body["user_list"]
        .as_array()
        .expect("response should contain a user_list array");

    // Owner + the second member.
    assert_eq!(user_list.len(), 2, "expected 2 related users: {body}");

    let emails: Vec<&str> = user_list
        .iter()
        .map(|u| u["email"].as_str().unwrap())
        .collect();
    assert!(emails.contains(&"test@example.com"), "emails: {emails:?}");
    assert!(emails.contains(&"second@example.com"), "emails: {emails:?}");

    for u in user_list {
        assert!(u["email"].as_str().is_some(), "email missing: {u}");
        assert!(u["name"].as_str().is_some(), "name missing: {u}");
        assert!(
            u["contact_email"].as_str().is_some(),
            "contact_email missing: {u}"
        );
        assert!(
            u["avatar_url"].as_str().is_some(),
            "avatar_url missing: {u}"
        );
    }
}
