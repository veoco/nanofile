//! Authentication and authorization tests for unified API keys.
//!
//! These pin the behaviour that makes a key a *scoped* credential: it has to
//! carry the capability a route needs, cover the library it names, and it can
//! never manage other keys. Each case also checks the negative side, because a
//! fail-open regression here is silent.

mod common;

use common::TestFixture;
use serde_json::{Value, json};
use server::repository::api_key::UpdateApiKeyParams;

/// Mint a key through the management API and return its plaintext.
async fn create_key(f: &TestFixture, body: &Value) -> String {
    let resp = f
        .client
        .post_json("/api2/api-keys/", Some(&f.api_token), body)
        .await;
    assert_eq!(resp.status(), 200, "key creation failed");
    resp.json::<Value>().await.unwrap()["key"]
        .as_str()
        .unwrap()
        .to_string()
}

fn key_body(capabilities: &[&str], all_repos: bool, repos: &[(&str, &str)]) -> Value {
    json!({
        "name": "test-key",
        "capabilities": capabilities,
        "all_repos": all_repos,
        "repo_permissions": repos
            .iter()
            .map(|(repo_id, permission)| json!({"repo_id": repo_id, "permission": permission}))
            .collect::<Vec<_>>(),
        "expires_in_days": 7,
    })
}

async fn second_repo(f: &TestFixture) -> String {
    let resp = f.client.create_repo(&f.api_token, "second").await;
    assert_eq!(resp.status(), 201);
    resp.json::<Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn a_key_exercises_only_the_capabilities_it_holds() {
    let f = TestFixture::new().await;
    let key = create_key(
        &f,
        &key_body(&["library.read", "file.read"], false, &[(&f.repo_id, "rw")]),
    )
    .await;

    let resp = f.client.get("/api2/repos/", Some(&key)).await;
    assert_eq!(resp.status(), 200, "library.read should list libraries");

    let resp = f
        .client
        .get(&format!("/api2/repos/{}/dir/?p=/", f.repo_id), Some(&key))
        .await;
    assert_eq!(resp.status(), 200, "file.read should list a directory");

    // No file.write capability: creating a directory must be refused.
    let resp = f
        .client
        .post_form(
            &format!("/api2/repos/{}/dir/?p=/", f.repo_id),
            Some(&key),
            &[("operation", "mkdir"), ("path", "/nope")],
        )
        .await;
    assert_eq!(resp.status(), 403, "file.write was not granted");

    // Nor may it reach a surface it was not granted at all.
    let resp = f.client.get("/api2/starredfiles/", Some(&key)).await;
    assert_eq!(resp.status(), 403, "star.read was not granted");
}

#[tokio::test]
async fn a_read_only_key_cannot_mint_credentials() {
    let f = TestFixture::new().await;
    let key = create_key(
        &f,
        &key_body(&["library.read", "file.read"], false, &[(&f.repo_id, "rw")]),
    )
    .await;

    // A sync token is a full read/write library credential, so minting one
    // needs `sync.token`.
    let resp = f.client.download_info(&key, &f.repo_id).await;
    assert_eq!(resp.status(), 403, "sync.token was not granted");

    // Capability URLs are write-level for the same reason.
    for path in [
        format!("/api2/repos/{}/upload-link/", f.repo_id),
        format!("/api2/repos/{}/update-link/", f.repo_id),
        format!("/api2/repos/{}/upload-blks-link/", f.repo_id),
    ] {
        let resp = f.client.get(&path, Some(&key)).await;
        assert_eq!(resp.status(), 403, "{path} needs a write capability");
    }

    // A read of the same library still works.
    let resp = f
        .client
        .get(&format!("/api2/repos/{}/", f.repo_id), Some(&key))
        .await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn a_bound_key_cannot_reach_another_library() {
    let f = TestFixture::new().await;
    let other = second_repo(&f).await;
    let key = create_key(
        &f,
        &key_body(
            &[
                "library.read",
                "file.read",
                "file.write",
                "trash.read",
                "sync.token",
            ],
            false,
            &[(&f.repo_id, "rw")],
        ),
    )
    .await;

    // Direct access through a path-carried library id.
    for path in [
        format!("/api2/repos/{other}/"),
        format!("/api2/repos/{other}/dir/?p=/"),
        format!("/api/v2.1/repos/{other}/trash/"),
    ] {
        let resp = f.client.get(&path, Some(&key)).await;
        assert_eq!(resp.status(), 403, "{path} is outside the key's scope");
    }

    // A write through the same path is refused too.
    let resp = f
        .client
        .post_form(
            &format!("/api2/repos/{other}/dir/?p=/"),
            Some(&key),
            &[("operation", "mkdir"), ("path", "/nope")],
        )
        .await;
    assert_eq!(resp.status(), 403);

    // The account-wide listings must not describe it either.
    let listed: Vec<Value> = f
        .client
        .get("/api2/repos/", Some(&key))
        .await
        .json()
        .await
        .unwrap();
    let ids: Vec<&str> = listed
        .iter()
        .map(|repo| repo["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&f.repo_id.as_str()));
    assert!(!ids.contains(&other.as_str()), "leaked a library id");

    let resp = f
        .client
        .get(
            &format!("/api2/repo-tokens/?repos={},{}", f.repo_id, other),
            Some(&key),
        )
        .await;
    assert_eq!(resp.status(), 200, "sync.token was granted");
    let tokens: Value = resp.json().await.unwrap();
    let token_map = tokens.as_object().unwrap();
    assert!(token_map.contains_key(&f.repo_id));
    assert!(
        !token_map.contains_key(&other),
        "minted a sync token for a library outside the scope"
    );
}

#[tokio::test]
async fn an_all_repos_key_reaches_every_library() {
    let f = TestFixture::new().await;
    let other = second_repo(&f).await;
    let key = create_key(&f, &key_body(&["library.read", "file.read"], true, &[])).await;

    let listed: Vec<Value> = f
        .client
        .get("/api2/repos/", Some(&key))
        .await
        .json()
        .await
        .unwrap();
    let ids: Vec<&str> = listed
        .iter()
        .map(|repo| repo["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&f.repo_id.as_str()));
    assert!(ids.contains(&other.as_str()));

    for repo_id in [&f.repo_id, &other] {
        let resp = f
            .client
            .get(&format!("/api2/repos/{repo_id}/dir/?p=/"), Some(&key))
            .await;
        assert_eq!(resp.status(), 200, "all-repos keys cover every library");
    }
}

#[tokio::test]
async fn a_per_library_ceiling_downgrades_writes() {
    let f = TestFixture::new().await;
    // The key holds file.write, but only read for this library.
    let key = create_key(
        &f,
        &key_body(
            &["library.read", "file.read", "file.write"],
            false,
            &[(&f.repo_id, "r")],
        ),
    )
    .await;

    let resp = f
        .client
        .get(&format!("/api2/repos/{}/dir/?p=/", f.repo_id), Some(&key))
        .await;
    assert_eq!(resp.status(), 200, "reads stay allowed");

    let resp = f
        .client
        .post_form(
            &format!("/api2/repos/{}/dir/?p=/", f.repo_id),
            Some(&key),
            &[("operation", "mkdir"), ("path", "/nope")],
        )
        .await;
    assert_eq!(resp.status(), 403, "the read ceiling blocks the write");
}

#[tokio::test]
async fn a_key_cannot_manage_keys() {
    let f = TestFixture::new().await;
    let key = create_key(
        &f,
        &key_body(&["library.read", "file.read", "account.read"], true, &[]),
    )
    .await;

    let resp = f.client.get("/api2/api-keys/", Some(&key)).await;
    assert_eq!(resp.status(), 403, "a key must not list keys");

    let resp = f
        .client
        .post_json(
            "/api2/api-keys/",
            Some(&key),
            &key_body(&["library.read"], true, &[]),
        )
        .await;
    assert_eq!(resp.status(), 403, "a key must not mint keys");

    let resp = f.client.get("/api2/api-keys/catalog/", Some(&key)).await;
    assert_eq!(resp.status(), 403, "the catalog is part of the key surface");
}

#[tokio::test]
async fn an_expired_key_is_rejected() {
    let f = TestFixture::new().await;
    let key = create_key(&f, &key_body(&["library.read"], true, &[])).await;
    let keys: Value = f
        .client
        .get("/api2/api-keys/", Some(&f.api_token))
        .await
        .json()
        .await
        .unwrap();
    let key_id = keys["keys"][0]["id"].as_i64().unwrap();

    // Expire it directly: the API refuses to create an already-expired key, so
    // this is the only way to reach the state.
    f.server
        .repos
        .api_key
        .update_metadata(
            key_id as i32,
            f.user_id,
            UpdateApiKeyParams {
                expires_at: Some(Some(chrono::Utc::now().timestamp() - 1)),
                ..Default::default()
            },
        )
        .await
        .expect("expire the key");

    let resp = f.client.get("/api2/repos/", Some(&key)).await;
    assert_eq!(resp.status(), 401, "an expired key is not a credential");
}

#[tokio::test]
async fn sync_access_follows_the_sync_capabilities() {
    let f = TestFixture::new().await;

    let read_key = create_key(
        &f,
        &key_body(
            &["library.read", "file.read", "sync.read"],
            false,
            &[(&f.repo_id, "rw")],
        ),
    )
    .await;
    let resp = f.client.get_head_commit(&read_key, &f.repo_id).await;
    assert_eq!(resp.status(), 200, "sync.read should allow a sync read");

    // Without the capability the same request is refused, and the status stays
    // 403 because seaf-daemon only understands 403/400 here.
    let no_sync_key = create_key(
        &f,
        &key_body(&["library.read", "file.read"], false, &[(&f.repo_id, "rw")]),
    )
    .await;
    let resp = f.client.get_head_commit(&no_sync_key, &f.repo_id).await;
    assert_eq!(resp.status(), 403, "sync.read was not granted");

    // A sync-capable key bound to this library cannot be used on another.
    let other = second_repo(&f).await;
    let resp = f.client.get_head_commit(&read_key, &other).await;
    assert_eq!(resp.status(), 403, "the sync scope is per library");
}

#[tokio::test]
async fn sessions_are_unaffected_by_the_key_guard() {
    let f = TestFixture::new().await;
    // The unclassified-route deny and the capability checks apply to keys only:
    // a login token still reaches every endpoint it always did.
    for path in [
        "/api2/repos/",
        "/api2/starredfiles/",
        "/api2/api-keys/",
        "/api/v2.1/activities/",
    ] {
        let resp = f.client.get(path, Some(&f.api_token)).await;
        assert_eq!(resp.status(), 200, "session access to {path}");
    }
}
