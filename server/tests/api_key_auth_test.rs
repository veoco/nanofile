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

    let resp = f
        .client
        .post_form(
            &format!("/api2/repos/{}/dir/?p=/", f.repo_id),
            Some(&key),
            &[("operation", "mkdir"), ("path", "/nope")],
        )
        .await;
    assert_eq!(resp.status(), 403, "file.write was not granted");

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

    for path in [
        format!("/api2/repos/{other}/"),
        format!("/api2/repos/{other}/dir/?p=/"),
        format!("/api/v2.1/repos/{other}/trash/"),
    ] {
        let resp = f.client.get(&path, Some(&key)).await;
        assert_eq!(resp.status(), 403, "{path} is outside the key's scope");
    }

    let resp = f
        .client
        .post_form(
            &format!("/api2/repos/{other}/dir/?p=/"),
            Some(&key),
            &[("operation", "mkdir"), ("path", "/nope")],
        )
        .await;
    assert_eq!(resp.status(), 403);

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

/// The route-table scope check only reads the URI. These endpoints put the
/// library id in the JSON body or query string, so a bound key would otherwise
/// reach any library its owner can access. All of them must refuse the request
/// and leave the other library untouched.
#[tokio::test]
async fn body_and_query_repo_ids_cannot_escape_a_bound_key() {
    let f = TestFixture::new().await;
    let other = second_repo(&f).await;
    let key = create_key(
        &f,
        &key_body(
            &[
                "file.read",
                "file.write",
                "library.read",
                "library.delete",
                "share_link.write",
                "upload_link.write",
                "search.reindex",
                "search.write",
                "star.read",
                "star.write",
                "device.read",
                "device.write",
            ],
            false,
            &[(&f.repo_id, "rw")],
        ),
    )
    .await;

    // Seed content in B so a leaked write would be observable.
    let upload = f
        .client
        .upload_file(&f.api_token, &other, "/", "keep.txt", b"original")
        .await;
    assert!(upload.status().is_success(), "seed B file");
    let mkdir = f.client.create_dir(&f.api_token, &other, "/keepdir").await;
    assert_eq!(mkdir.status(), 201, "seed B directory");

    // The path says A; the body says B. The key covers A only, so the body
    // cannot be used to smuggle B past the guard. The handler rejects the
    // mismatch before any mutation.
    let resp = f
        .client
        .post_json(
            &format!("/api2/repos/{}/file/move/", f.repo_id),
            Some(&key),
            &json!({"repo_id": other, "p": "/keep.txt", "new_parent_dir": "/moved"}),
        )
        .await;
    assert_eq!(resp.status(), 400, "file move path/body mismatch");

    let resp = f
        .client
        .post_json(
            &format!("/api2/repos/{}/file/rename/", f.repo_id),
            Some(&key),
            &json!({"repo_id": other, "p": "/keep.txt", "new_name": "renamed.txt"}),
        )
        .await;
    assert_eq!(resp.status(), 400, "file rename path/body mismatch");

    let resp = f
        .client
        .post_json(
            &format!("/api2/repos/{}/dir/move/", f.repo_id),
            Some(&key),
            &json!({"repo_id": other, "p": "/keepdir", "new_parent_dir": "/moved"}),
        )
        .await;
    assert_eq!(resp.status(), 400, "dir move path/body mismatch");

    let resp = f
        .client
        .post_json(
            &format!("/api2/repos/{}/dir/rename/", f.repo_id),
            Some(&key),
            &json!({"repo_id": other, "p": "/keepdir", "new_name": "renameddir"}),
        )
        .await;
    assert_eq!(resp.status(), 400, "dir rename path/body mismatch");

    let listing = f
        .client
        .list_dir(&f.api_token, &other, "/")
        .await
        .json::<Value>()
        .await
        .unwrap()
        .to_string();
    assert!(
        listing.contains("keep.txt"),
        "B file disappeared: {listing}"
    );
    assert!(
        listing.contains("keepdir"),
        "B directory disappeared: {listing}"
    );
    assert!(
        !listing.contains("renamed.txt") && !listing.contains("renameddir"),
        "a move/rename escaped its scope: {listing}"
    );
    let content = f
        .client
        .download_file(&f.api_token, &other, "/keep.txt")
        .await
        .bytes()
        .await
        .unwrap();
    assert_eq!(content.as_ref(), b"original");

    // Creation endpoints take the repo id from the body only.
    for path in [
        "/api/v2.1/share-links/",
        "/api/v2.1/multi-share-links/",
        "/api2/shared-links",
    ] {
        let resp = f
            .client
            .post_json(path, Some(&key), &json!({"repo_id": other, "path": "/"}))
            .await;
        assert_eq!(resp.status(), 403, "{path} created a link for B");
    }
    for path in ["/api/v2.1/upload-links/", "/api2/upload-links"] {
        let resp = f
            .client
            .post_json(path, Some(&key), &json!({"repo_id": other, "path": "/"}))
            .await;
        assert_eq!(resp.status(), 403, "{path} created a link for B");
    }

    let shares: Value = f
        .client
        .get("/api/v2.1/share-links/", Some(&f.api_token))
        .await
        .json()
        .await
        .unwrap();
    assert!(
        !shares.to_string().contains(&other),
        "a share link for B was persisted: {shares}"
    );
    let uploads: Value = f
        .client
        .get("/api/v2.1/upload-links/", Some(&f.api_token))
        .await
        .json()
        .await
        .unwrap();
    assert!(
        !uploads.to_string().contains(&other),
        "an upload link for B was persisted: {uploads}"
    );

    // The two account-wide trash surfaces used to list/purge every library the
    // user owned, regardless of the key's scope.
    let deleted = f.client.delete_repo(&f.api_token, &other).await;
    assert!(deleted.status().is_success(), "delete B into trash");

    let trashed: Value = f
        .client
        .get("/api/v2.1/deleted-repos/", Some(&key))
        .await
        .json()
        .await
        .unwrap();
    assert!(
        !trashed.to_string().contains(&other),
        "the key listed B's trash entry: {trashed}"
    );

    let resp = f
        .client
        .post_form(
            "/api/v2.1/deleted-repos/",
            Some(&key),
            &[("repo_id", other.as_str())],
        )
        .await;
    assert_eq!(resp.status(), 403, "restore B outside the key scope");

    let resp = f
        .client
        .delete("/api/v2.1/deleted-repos/", Some(&key))
        .await;
    assert_eq!(resp.status(), 200, "empty-trash request itself is allowed");
    let still_trashed: Value = f
        .client
        .get("/api/v2.1/deleted-repos/", Some(&f.api_token))
        .await
        .json()
        .await
        .unwrap();
    assert!(
        still_trashed.to_string().contains(&other),
        "the key permanently deleted B: {still_trashed}"
    );

    let restored = f
        .client
        .post_form(
            "/api/v2.1/deleted-repos/",
            Some(&f.api_token),
            &[("repo_id", other.as_str())],
        )
        .await;
    assert_eq!(restored.status(), 200, "restore B as its owner");

    // Body/query index and metadata operations.
    let resp = f
        .client
        .post_json("/api2/reindex/", Some(&key), &json!({"repo_id": other}))
        .await;
    assert_eq!(resp.status(), 403, "reindex B outside the key scope");

    let resp = f
        .client
        .post_json(
            "/api2/index-file-text/",
            Some(&key),
            &json!({"repo_id": other, "path": "/keep.txt", "text": "escaped"}),
        )
        .await;
    assert_eq!(resp.status(), 403, "index text for B");

    let resp = f
        .client
        .post_json(
            "/api/v2.1/starred-items/",
            Some(&key),
            &json!({"repo_id": other, "path": "/keep.txt"}),
        )
        .await;
    assert_eq!(resp.status(), 403, "star B");

    let resp = f
        .client
        .delete(
            &format!("/api/v2.1/starred-items/?repo_id={other}&path=/keep.txt"),
            Some(&key),
        )
        .await;
    assert_eq!(resp.status(), 403, "unstar B");

    let starred: Value = f
        .client
        .get("/api/v2.1/starred-items/", Some(&f.api_token))
        .await
        .json()
        .await
        .unwrap();
    assert!(
        !starred.to_string().contains(&other),
        "B was starred through a bound key: {starred}"
    );

    let resp = f
        .client
        .get(
            &format!("/api/v2.1/smart-link/?repo_id={other}&path=/keep.txt"),
            Some(&key),
        )
        .await;
    assert_eq!(resp.status(), 403, "smart link for B");
}

/// A share/upload token is a capability URL. A key bound to A must not be able
/// to read, update or delete a token that belongs to B, even though both links
/// have the same owner.
#[tokio::test]
async fn link_tokens_are_scoped_to_the_key_library() {
    let f = TestFixture::new().await;
    let other = second_repo(&f).await;
    let key = create_key(
        &f,
        &key_body(
            &[
                "share_link.read",
                "share_link.write",
                "upload_link.read",
                "upload_link.write",
            ],
            false,
            &[(&f.repo_id, "rw")],
        ),
    )
    .await;

    let share: Value = f
        .client
        .post_json(
            "/api/v2.1/share-links/",
            Some(&f.api_token),
            &json!({"repo_id": other, "path": "/"}),
        )
        .await
        .json()
        .await
        .unwrap();
    let share_token = share["token"].as_str().unwrap().to_string();

    let upload: Value = f
        .client
        .post_json(
            "/api/v2.1/upload-links/",
            Some(&f.api_token),
            &json!({"repo_id": other, "path": "/"}),
        )
        .await
        .json()
        .await
        .unwrap();
    let upload_token = upload["token"].as_str().unwrap().to_string();

    let share_path = format!("/api/v2.1/share-links/{share_token}/");
    let resp = f.client.get(&share_path, Some(&key)).await;
    assert_eq!(resp.status(), 404, "read B share link");
    let resp = f
        .client
        .put_json(&share_path, Some(&key), &json!({"description": "x"}))
        .await;
    assert_eq!(resp.status(), 404, "update B share link");

    let upload_path = format!("/api/v2.1/upload-links/{upload_token}/");
    let resp = f.client.get(&upload_path, Some(&key)).await;
    assert_eq!(resp.status(), 404, "read B upload link");
    let resp = f
        .client
        .put_json(&upload_path, Some(&key), &json!({"description": "x"}))
        .await;
    assert_eq!(resp.status(), 404, "update B upload link");

    let resp = f.client.get(&share_path, Some(&f.api_token)).await;
    assert_eq!(resp.status(), 200, "owner can still read the share link");
    let resp = f.client.get(&upload_path, Some(&f.api_token)).await;
    assert_eq!(resp.status(), 200, "owner can still read the upload link");

    let resp = f.client.delete(&share_path, Some(&key)).await;
    assert_eq!(resp.status(), 404, "delete B share link");
    let resp = f.client.delete(&upload_path, Some(&key)).await;
    assert_eq!(resp.status(), 404, "delete B upload link");

    let resp = f.client.get(&share_path, Some(&f.api_token)).await;
    assert_eq!(
        resp.status(),
        200,
        "failed delete must not remove the share link"
    );
    let resp = f.client.get(&upload_path, Some(&f.api_token)).await;
    assert_eq!(
        resp.status(),
        200,
        "failed delete must not remove the upload link"
    );

    // The v1 delete routes carry the same tokens and need the same guard.
    let resp = f
        .client
        .delete(&format!("/api2/shared-links/{share_token}"), Some(&key))
        .await;
    assert_eq!(resp.status(), 404, "v1 delete B share link");
    let resp = f
        .client
        .delete(&format!("/api2/upload-links/{upload_token}"), Some(&key))
        .await;
    assert_eq!(resp.status(), 404, "v1 delete B upload link");
}

/// Sync-token inventory is account-wide by default; a bound key must see only
/// its own libraries and must not revoke another library's token by id.
#[tokio::test]
async fn credential_inventory_and_sync_token_revoke_are_scoped() {
    let f = TestFixture::new().await;
    let other = second_repo(&f).await;
    let key = create_key(
        &f,
        &key_body(
            &["device.read", "device.write"],
            false,
            &[(&f.repo_id, "rw")],
        ),
    )
    .await;

    let info = f.client.download_info(&f.api_token, &other).await;
    assert_eq!(info.status(), 200, "mint a sync token for B");
    let info: Value = info.json().await.unwrap();
    assert_eq!(info["repo_id"].as_str(), Some(other.as_str()));

    let inventory: Value = f
        .client
        .get("/api2/credentials/", Some(&key))
        .await
        .json()
        .await
        .unwrap();
    assert!(
        !inventory.to_string().contains(&other),
        "the key saw B's sync-token metadata: {inventory}"
    );

    let devices: Value = f
        .client
        .get("/api2/devices/", Some(&key))
        .await
        .json()
        .await
        .unwrap();
    assert!(
        !devices.to_string().contains(&other),
        "the key saw B in the device inventory: {devices}"
    );

    let owner_inventory: Value = f
        .client
        .get("/api2/credentials/", Some(&f.api_token))
        .await
        .json()
        .await
        .unwrap();
    let token_id = owner_inventory["sync_tokens"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["repo_id"] == other.as_str())
        .expect("the owner sees B's sync token")["id"]
        .as_i64()
        .unwrap();

    let resp = f
        .client
        .delete(
            &format!("/api2/credentials/sync_token/{token_id}/"),
            Some(&key),
        )
        .await;
    assert_eq!(resp.status(), 404, "revoke B's sync token by id");

    let owner_after: Value = f
        .client
        .get("/api2/credentials/", Some(&f.api_token))
        .await
        .json()
        .await
        .unwrap();
    assert!(
        owner_after["sync_tokens"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["id"].as_i64() == Some(token_id)),
        "the failed revoke removed B's sync token: {owner_after}"
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

    let other = second_repo(&f).await;
    let resp = f.client.get_head_commit(&read_key, &other).await;
    assert_eq!(resp.status(), 403, "the sync scope is per library");
}

/// "When was this key last used?" is only answerable if using it records that.
/// The web UI rendered a column nothing ever wrote, because only the WebDAV
/// path touched it.
#[tokio::test]
async fn using_a_key_records_when_it_was_last_used() {
    let f = TestFixture::new().await;
    // `sync.read` as well, so the same key can be used on both surfaces.
    let key = create_key(&f, &key_body(&["library.read", "sync.read"], true, &[])).await;

    async fn last_used(f: &TestFixture, key_id: i64) -> Option<i64> {
        let listed: Value = f
            .client
            .get("/api2/api-keys/", Some(&f.api_token))
            .await
            .json()
            .await
            .unwrap();
        listed["keys"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["id"].as_i64() == Some(key_id))
            .expect("the key")["last_used_at"]
            .as_i64()
    }

    let listed: Value = f
        .client
        .get("/api2/api-keys/", Some(&f.api_token))
        .await
        .json()
        .await
        .unwrap();
    let key_id = listed["keys"][0]["id"].as_i64().expect("id");
    assert_eq!(
        last_used(&f, key_id).await,
        None,
        "a key that has never been presented has no last use"
    );

    let resp = f.client.get("/api2/repos/", Some(&key)).await;
    assert_eq!(resp.status(), 200);
    let after_rest = last_used(&f, key_id).await.expect("recorded by /api2");

    let resp = f.client.get_head_commit(&key, &f.repo_id).await;
    assert_eq!(resp.status(), 200);
    let after_sync = last_used(&f, key_id).await.expect("recorded by /seafhttp");

    assert!(after_rest > 0);
    assert!(after_sync >= after_rest);
}

#[tokio::test]
async fn a_session_satisfies_every_capability_but_is_still_classified() {
    let f = TestFixture::new().await;
    // The route table says what a *key* needs. A session is the account itself,
    // so it satisfies every capability — including on the session-only
    // key-management surface, which is the one place a key is refused.
    for path in [
        "/api2/repos/",
        "/api2/starredfiles/",
        "/api2/api-keys/",
        "/api2/api-keys/catalog/",
        "/api2/account/info/",
        "/api2/devices/",
        "/api2/groups/",
        "/api2/unseen_messages/",
        "/api/v2.1/activities/",
    ] {
        let resp = f.client.get(path, Some(&f.api_token)).await;
        assert_eq!(resp.status(), 200, "session access to {path}");
    }

    // Library-scoped routes are classified too, and a session has no ceiling.
    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/dir/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200, "session access to a library route");
}

/// A read-only ceiling is not only about file writes: an upload link delegates
/// anonymous write access, and emptying the trash permanently deletes content.
/// Both must respect the per-library `r` binding.
#[tokio::test]
async fn a_read_ceiling_cannot_mint_upload_links() {
    let f = TestFixture::new().await;
    let key = create_key(
        &f,
        &key_body(
            &[
                "library.read",
                "library.delete",
                "trash.read",
                "upload_link.write",
            ],
            false,
            &[(&f.repo_id, "r")],
        ),
    )
    .await;

    for path in ["/api/v2.1/upload-links/", "/api2/upload-links"] {
        let resp = f
            .client
            .post_json(
                path,
                Some(&key),
                &json!({"repo_id": f.repo_id, "path": "/"}),
            )
            .await;
        assert_eq!(resp.status(), 403, "{path} must honour the read ceiling");
    }
}
