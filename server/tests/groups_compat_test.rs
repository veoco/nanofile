//! Guard the group routes that remain, and the absence of the ones that do not.
//!
//! nanofile has no groups and no user-to-user library sharing; the two group
//! routes that survive are stateless stubs, kept because an official client
//! fails on a load path without them (see `handler::groups`). This file pins
//! both halves: the stub must answer 200 with the shape the clients parse, and
//! every removed route must be gone rather than silently answering something
//! else.

mod common;

use common::TestFixture;

/// `GET /api2/groups/` — the desktop client's group-share dialog fetches this.
/// It must be a top-level array and an empty one.
#[tokio::test]
async fn test_groups_empty() {
    let f = TestFixture::new().await;
    let resp = f.client.get("/api2/groups/", Some(&f.api_token)).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_groups_unauthorized() {
    let server = common::TestServer::start().await;
    let client = server.client();
    let resp = client.get("/api2/groups/", None).await;
    assert_eq!(resp.status(), 401);
}

/// v2.1 — `GET /api/v2.1/groups/`, required by seadroid's `getGroupsAsync` in
/// the library-list load chain. It must return 200 + a JSON array, never 404:
/// the client subscribes to it before the library list, and a rejection clears
/// the cached list and shows "Error when loading libraries".
#[tokio::test]
async fn test_groups_v21_unauthorized() {
    let server = common::TestServer::start().await;
    let client = server.client();
    let resp = client.get("/api/v2.1/groups/", None).await;
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_groups_v21_is_an_empty_array() {
    let f = TestFixture::new().await;
    for query in ["", "?with_repos=0", "?with_repos=1"] {
        let resp = f
            .client
            .get(&format!("/api/v2.1/groups/{query}"), Some(&f.api_token))
            .await;
        assert_eq!(resp.status(), 200, "GET /api/v2.1/groups/{query}");
        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(
            body.as_array().expect("a top-level array").is_empty(),
            "the groups stub is always empty"
        );
    }
}

#[tokio::test]
async fn test_groups_v21_rejects_an_invalid_with_repos() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .get("/api/v2.1/groups/?with_repos=2", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 400);
}

/// The routes that carried groups and user-to-user sharing are gone.
#[tokio::test]
async fn removed_share_and_group_routes_are_not_mounted() {
    let f = TestFixture::new().await;
    let token = Some(f.api_token.as_str());
    let repo = f.repo_id.as_str();

    let removed: [(&str, &str); 8] = [
        ("GET", "/api2/groupandcontacts/"),
        ("GET", "/api2/search-user/?q=someone"),
        ("GET", "/api2/beshared-repos/{repo}/"),
        ("POST", "/api2/beshared-repos/{repo}/"),
        ("PUT", "/api2/beshared-repos/{repo}/"),
        ("DELETE", "/api2/beshared-repos/{repo}/"),
        ("GET", "/api/v2.1/repos/{repo}/custom-share-permissions/"),
        (
            "GET",
            "/api/v2.1/repos/{repo}/custom-share-permissions/{repo}/",
        ),
    ];

    for (method, path) in removed {
        let path = path.replace("{repo}", repo);
        let resp = match method {
            "GET" => f.client.get(&path, token).await,
            "POST" => {
                f.client
                    .post_json(&path, token, &serde_json::json!({}))
                    .await
            }
            "PUT" => {
                f.client
                    .put_json(&path, token, &serde_json::json!({}))
                    .await
            }
            "DELETE" => {
                f.client
                    .delete_json(&path, token, &serde_json::json!({}))
                    .await
            }
            other => panic!("unhandled method {other}"),
        };
        assert_eq!(resp.status(), 404, "{method} {path} must be unmounted");
    }
}

/// `related-users` is *not* part of the removed surface: seadroid zips it into
/// the file-detail load, so a 404 there would break the profile dialog. It now
/// reports the library's owner, since only owner rows remain.
#[tokio::test]
async fn test_related_users_still_answers() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .get(
            &format!("/api/v2.1/repos/{}/related-users/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let users = body["user_list"].as_array().expect("user_list");
    assert_eq!(users.len(), 1, "only the owner is related to the library");
    assert_eq!(users[0]["email"].as_str(), Some(f.email.as_str()));
}
