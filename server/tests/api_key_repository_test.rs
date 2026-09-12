//! Repository-level tests for the unified API-key store.
//!
//! These drive the data layer directly (no HTTP surface exists yet for keys);
//! they pin the invariants the auth path depends on: lookup by presented
//! secret, atomic bindings, immediate cache invalidation, and orphan cleanup.

mod common;

use common::TestFixture;
use server::repository::api_key::{ApiKeyBinding, CreateApiKeyParams, UpdateApiKeyParams};

const RAW: &str = "0123456789abcdef0123456789abcdef01234567";

fn create_params(f: &TestFixture, raw: &str, name: &str) -> CreateApiKeyParams {
    CreateApiKeyParams {
        user_id: f.user_id,
        name: name.to_string(),
        key_hash: server::service::auth::token::hash_token(raw),
        key_prefix: raw[..8].to_string(),
        capabilities: "file.read,file.write".to_string(),
        all_repos: false,
        created_at: chrono::Utc::now().timestamp(),
        expires_at: None,
        bindings: vec![ApiKeyBinding {
            repo_id: f.repo_id.clone(),
            permission: "rw".to_string(),
        }],
    }
}

#[tokio::test]
async fn create_and_look_up_a_key_with_its_bindings() {
    let f = TestFixture::new().await;
    let repos = &f.server.repos;

    let created = repos
        .api_key
        .create(create_params(&f, RAW, "ci"))
        .await
        .expect("create key");
    assert_eq!(created.key_prefix.as_deref(), Some("01234567"));
    assert!(!created.all_repos);

    let lookup = repos
        .api_key
        .find_by_presented(RAW)
        .await
        .expect("lookup")
        .expect("key exists");
    assert_eq!(lookup.key.id, created.id);
    assert_eq!(lookup.key.capabilities, "file.read,file.write");
    assert_eq!(lookup.bindings.len(), 1);
    assert_eq!(lookup.bindings[0].repo_id, f.repo_id);
    assert_eq!(lookup.bindings[0].permission, "rw");

    // The stored value is a hash: the plaintext must not be recoverable.
    assert_ne!(lookup.key.key_hash, RAW);
    assert_eq!(
        lookup.key.key_hash,
        server::service::auth::token::hash_token(RAW)
    );

    assert!(
        repos
            .api_key
            .find_by_presented("deadbeef")
            .await
            .expect("lookup")
            .is_none(),
        "an unknown secret must not resolve"
    );
}

#[tokio::test]
async fn revocation_takes_effect_immediately() {
    let f = TestFixture::new().await;
    let repos = &f.server.repos;

    let created = repos
        .api_key
        .create(create_params(&f, RAW, "ci"))
        .await
        .expect("create key");
    // Populate the lookup cache, then revoke.
    assert!(
        repos
            .api_key
            .find_by_presented(RAW)
            .await
            .unwrap()
            .is_some()
    );

    assert!(
        repos
            .api_key
            .delete_by_id_and_user(created.id, f.user_id)
            .await
            .expect("delete")
    );
    assert!(
        repos
            .api_key
            .find_by_presented(RAW)
            .await
            .expect("lookup")
            .is_none(),
        "a revoked key must stop resolving without waiting for the cache TTL"
    );
    // A different user cannot revoke someone else's key.
    let other = repos
        .api_key
        .create(create_params(&f, RAW, "ci"))
        .await
        .expect("recreate");
    assert!(
        !repos
            .api_key
            .delete_by_id_and_user(other.id, f.user_id + 1000)
            .await
            .expect("delete")
    );
    assert!(
        repos
            .api_key
            .find_by_presented(RAW)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn binding_changes_apply_without_waiting_for_the_cache() {
    let f = TestFixture::new().await;
    let repos = &f.server.repos;

    let created = repos
        .api_key
        .create(create_params(&f, RAW, "ci"))
        .await
        .expect("create key");
    assert!(
        repos
            .api_key
            .find_by_presented(RAW)
            .await
            .unwrap()
            .is_some()
    );

    repos
        .api_key
        .replace_bindings(created.id, &[])
        .await
        .expect("clear bindings");
    let lookup = repos
        .api_key
        .find_by_presented(RAW)
        .await
        .unwrap()
        .expect("key still exists");
    assert!(
        lookup.bindings.is_empty(),
        "the widened/narrowed scope must be visible at once"
    );

    // A key that is not `all_repos` and has no bindings left is inert, so the
    // cleanup pass removes it.
    assert_eq!(
        repos
            .api_key
            .delete_orphan_bound_keys()
            .await
            .expect("cleanup"),
        1
    );
    assert!(
        repos
            .api_key
            .find_by_presented(RAW)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn all_repos_keys_survive_orphan_cleanup() {
    let f = TestFixture::new().await;
    let repos = &f.server.repos;

    let mut params = create_params(&f, RAW, "account");
    params.all_repos = true;
    params.bindings = Vec::new();
    repos.api_key.create(params).await.expect("create key");

    assert_eq!(
        repos
            .api_key
            .delete_orphan_bound_keys()
            .await
            .expect("cleanup"),
        0,
        "an all-repos key legitimately has no binding rows"
    );
}

#[tokio::test]
async fn metadata_updates_are_scoped_to_the_owner() {
    let f = TestFixture::new().await;
    let repos = &f.server.repos;

    let created = repos
        .api_key
        .create(create_params(&f, RAW, "ci"))
        .await
        .expect("create key");

    assert!(
        repos
            .api_key
            .update_metadata(
                created.id,
                f.user_id,
                UpdateApiKeyParams {
                    name: Some("renamed".to_string()),
                    capabilities: Some("file.read".to_string()),
                    all_repos: Some(true),
                    expires_at: Some(Some(1_900_000_000)),
                },
            )
            .await
            .expect("update")
    );
    let updated = repos
        .api_key
        .find_by_id_and_user(created.id, f.user_id)
        .await
        .unwrap()
        .expect("key");
    assert_eq!(updated.name, "renamed");
    assert_eq!(updated.capabilities, "file.read");
    assert!(updated.all_repos);
    assert_eq!(updated.expires_at, Some(1_900_000_000));

    assert!(
        !repos
            .api_key
            .update_metadata(
                created.id,
                f.user_id + 1000,
                UpdateApiKeyParams {
                    name: Some("hijacked".to_string()),
                    ..Default::default()
                },
            )
            .await
            .expect("update")
    );
}

#[tokio::test]
async fn deleting_a_library_drops_its_bindings_and_unbound_keys() {
    let f = TestFixture::new().await;
    let repos = &f.server.repos;

    repos
        .api_key
        .create(create_params(&f, RAW, "ci"))
        .await
        .expect("create key");
    let mut second = create_params(&f, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "ci-2");
    second.all_repos = true;
    second.bindings = Vec::new();
    repos
        .api_key
        .create(second)
        .await
        .expect("create second key");

    assert_eq!(
        repos
            .api_key
            .delete_bindings_by_repo(&f.repo_id)
            .await
            .expect("drop bindings"),
        1
    );
    // The bound key lost its only library and is gone; the all-repos key stays.
    assert!(
        repos
            .api_key
            .find_by_presented(RAW)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        repos.api_key.find_by_user(f.user_id).await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn removing_a_member_drops_their_bindings_for_that_library() {
    let f = TestFixture::new().await;
    let repos = &f.server.repos;

    // The library owner's key must be untouched by another member's removal.
    repos
        .api_key
        .create(create_params(&f, RAW, "owner-key"))
        .await
        .expect("create owner key");

    // A second user with their own key on the same library.
    let other_id =
        common::create_test_user(f.server.db.as_ref(), "member@example.com", "password123").await;
    let other_raw = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    repos
        .api_key
        .create(CreateApiKeyParams {
            user_id: other_id,
            name: "member-key".to_string(),
            key_hash: server::service::auth::token::hash_token(other_raw),
            key_prefix: other_raw[..8].to_string(),
            capabilities: "file.read".to_string(),
            all_repos: false,
            created_at: chrono::Utc::now().timestamp(),
            expires_at: None,
            bindings: vec![ApiKeyBinding {
                repo_id: f.repo_id.clone(),
                permission: "r".to_string(),
            }],
        })
        .await
        .expect("create member key");

    assert_eq!(
        repos
            .api_key
            .delete_bindings_for_repo_user(&f.repo_id, other_id)
            .await
            .expect("drop bindings"),
        1
    );
    assert!(
        repos
            .api_key
            .find_by_presented(other_raw)
            .await
            .unwrap()
            .is_none(),
        "the removed member's key lost its only library"
    );
    let owner_lookup = repos
        .api_key
        .find_by_presented(RAW)
        .await
        .unwrap()
        .expect("the owner's key is untouched");
    assert_eq!(owner_lookup.bindings.len(), 1);
}

#[tokio::test]
async fn deleting_a_user_removes_their_keys() {
    let f = TestFixture::new().await;
    let repos = &f.server.repos;

    repos
        .api_key
        .create(create_params(&f, RAW, "ci"))
        .await
        .expect("create key");
    assert_eq!(
        repos
            .api_key
            .delete_by_user(f.user_id)
            .await
            .expect("delete all"),
        1
    );
    assert!(
        repos
            .api_key
            .find_by_user(f.user_id)
            .await
            .unwrap()
            .is_empty()
    );
}
