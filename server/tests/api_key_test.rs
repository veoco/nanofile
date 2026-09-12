//! HTTP-level tests for the unified API-key management endpoints.
//!
//! These cover the session-facing surface only: keys do not authenticate
//! anything yet, so every request here uses an ordinary login token.

mod common;

use common::{TestFixture, TestServer, create_test_repo, create_test_user};
use serde_json::{Value, json};

const KEYS: &str = "/api2/api-keys/";

fn bound_body(f: &TestFixture) -> Value {
    json!({
        "name": "ci",
        "capabilities": ["file.read", "file.write"],
        "repo_permissions": [{"repo_id": f.repo_id, "permission": "rw"}],
        "expires_in_days": 30,
    })
}

async fn create(f: &TestFixture, body: &Value) -> reqwest::Response {
    f.client.post_json(KEYS, Some(&f.api_token), body).await
}

#[tokio::test]
async fn create_list_get_and_revoke() {
    let f = TestFixture::new().await;

    let resp = create(&f, &bound_body(&f)).await;
    assert_eq!(resp.status(), 200, "create failed");
    let created: Value = resp.json().await.unwrap();
    let secret = created["key"].as_str().expect("plaintext key").to_string();
    assert_eq!(secret.len(), 40, "keys are 40 hex characters");
    assert!(secret.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(created["key_prefix"].as_str().unwrap(), &secret[..8]);
    assert_eq!(created["name"], "ci");
    assert!(!created["all_repos"].as_bool().unwrap());
    assert_eq!(created["repos"][0]["repo_id"], f.repo_id.as_str());
    assert_eq!(created["repos"][0]["permission"], "rw");
    assert!(created["expires_at"].as_i64().unwrap() > chrono::Utc::now().timestamp());
    let key_id = created["id"].as_i64().unwrap();

    // Capabilities are stored expanded, so a write grant implies read.
    let capabilities: Vec<String> = created["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_string())
        .collect();
    assert!(capabilities.contains(&"file.read".to_string()));
    assert!(capabilities.contains(&"file.write".to_string()));

    // Listing never includes the secret.
    let resp = f.client.get(KEYS, Some(&f.api_token)).await;
    assert_eq!(resp.status(), 200);
    let listed: Value = resp.json().await.unwrap();
    let keys = listed["keys"].as_array().unwrap();
    assert_eq!(keys.len(), 1);
    assert!(
        keys[0].get("key").is_none(),
        "list must not leak the secret"
    );

    let resp = f
        .client
        .get(&format!("{KEYS}{key_id}/"), Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);

    let resp = f
        .client
        .delete(&format!("{KEYS}{key_id}/"), Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.json::<Value>().await.unwrap()["success"], true);

    let resp = f
        .client
        .get(&format!("{KEYS}{key_id}/"), Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 404, "a revoked key must be gone");
}

#[tokio::test]
async fn an_explicit_lifetime_is_required() {
    let f = TestFixture::new().await;

    let mut body = bound_body(&f);
    body.as_object_mut().unwrap().remove("expires_in_days");
    assert_eq!(create(&f, &body).await.status(), 400, "no lifetime");

    let mut body = bound_body(&f);
    body["never"] = json!(true);
    assert_eq!(
        create(&f, &body).await.status(),
        400,
        "never plus expires_in_days is ambiguous"
    );

    let mut body = bound_body(&f);
    body["expires_in_days"] = json!(0);
    assert_eq!(create(&f, &body).await.status(), 400, "zero days");

    let mut body = bound_body(&f);
    body.as_object_mut().unwrap().remove("expires_in_days");
    body["expires_at"] = json!(chrono::Utc::now().timestamp() - 60);
    assert_eq!(create(&f, &body).await.status(), 400, "past expiry");

    let mut body = bound_body(&f);
    body.as_object_mut().unwrap().remove("expires_in_days");
    body["never"] = json!(true);
    assert_eq!(
        create(&f, &body).await.status(),
        200,
        "never is allowed while no bound is configured"
    );
}

#[tokio::test]
async fn capabilities_are_validated() {
    let f = TestFixture::new().await;

    let mut body = bound_body(&f);
    body["capabilities"] = json!([]);
    assert_eq!(
        create(&f, &body).await.status(),
        400,
        "empty capability set"
    );

    let mut body = bound_body(&f);
    body["capabilities"] = json!(["file.reed"]);
    assert_eq!(create(&f, &body).await.status(), 400, "unknown capability");

    let mut body = bound_body(&f);
    body["capabilities"] = json!(["key.write"]);
    assert_eq!(
        create(&f, &body).await.status(),
        400,
        "a key must never be able to manage keys"
    );

    let mut body = bound_body(&f);
    body["capabilities"] = json!(["admin.user.read"]);
    assert_eq!(
        create(&f, &body).await.status(),
        403,
        "admin capabilities require an admin"
    );
}

#[tokio::test]
async fn library_scope_is_validated() {
    let f = TestFixture::new().await;

    let mut body = bound_body(&f);
    body["all_repos"] = json!(true);
    assert_eq!(
        create(&f, &body).await.status(),
        400,
        "all_repos cannot carry bindings"
    );

    let mut body = bound_body(&f);
    body["repo_permissions"] = json!([]);
    assert_eq!(
        create(&f, &body).await.status(),
        400,
        "a restricted key needs at least one library"
    );

    let mut body = bound_body(&f);
    body["repo_permissions"] = json!([
        {"repo_id": f.repo_id, "permission": "rw"},
        {"repo_id": f.repo_id, "permission": "r"},
    ]);
    assert_eq!(create(&f, &body).await.status(), 400, "duplicate library");

    let mut body = bound_body(&f);
    body["repo_permissions"] = json!([{"repo_id": f.repo_id, "permission": "read"}]);

    assert_eq!(create(&f, &body).await.status(), 400, "bad permission");

    let mut body = bound_body(&f);
    body["repo_permissions"] =
        json!([{"repo_id": "00000000-0000-0000-0000-000000000000", "permission": "rw"}]);
    assert_eq!(create(&f, &body).await.status(), 404, "unknown library");

    let mut body = bound_body(&f);
    body.as_object_mut().unwrap().remove("repo_permissions");
    body["all_repos"] = json!(true);
    let resp = create(&f, &body).await;
    assert_eq!(resp.status(), 200);
    let created: Value = resp.json().await.unwrap();
    assert!(created["all_repos"].as_bool().unwrap());
    assert_eq!(created["repos"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn updates_rewrite_the_whole_scope() {
    let f = TestFixture::new().await;

    let created: Value = create(&f, &bound_body(&f)).await.json().await.unwrap();
    let key_id = created["id"].as_i64().unwrap();

    // Rename, narrow the capabilities and set a new expiry, without listing
    // libraries: the existing binding is kept.
    let resp = f
        .client
        .put_json(
            &format!("{KEYS}{key_id}/"),
            Some(&f.api_token),
            &json!({"name": "renamed", "capabilities": ["file.read"], "expires_in_days": 7}),
        )
        .await;
    assert_eq!(resp.status(), 200, "update failed");
    let updated: Value = resp.json().await.unwrap();
    assert_eq!(updated["name"], "renamed");
    assert_eq!(updated["capabilities"], json!(["file.read"]));
    assert_eq!(updated["repos"].as_array().unwrap().len(), 1);

    // Widening to every library drops the binding.
    let resp = f
        .client
        .put_json(
            &format!("{KEYS}{key_id}/"),
            Some(&f.api_token),
            &json!({"all_repos": true}),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let widened: Value = resp.json().await.unwrap();
    assert!(widened["all_repos"].as_bool().unwrap());
    assert_eq!(widened["repos"].as_array().unwrap().len(), 0);

    // Narrowing again without naming libraries would leave an inert key.
    let resp = f
        .client
        .put_json(
            &format!("{KEYS}{key_id}/"),
            Some(&f.api_token),
            &json!({"all_repos": false}),
        )
        .await;
    assert_eq!(resp.status(), 400);

    let resp = f
        .client
        .put_json(
            &format!("{KEYS}{key_id}/"),
            Some(&f.api_token),
            &json!({"all_repos": false, "repo_permissions": [{"repo_id": f.repo_id, "permission": "r"}]}),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let narrowed: Value = resp.json().await.unwrap();
    assert_eq!(narrowed["repos"][0]["permission"], "r");
}

#[tokio::test]
async fn keys_are_scoped_to_their_owner() {
    let f = TestFixture::new().await;
    let created: Value = create(&f, &bound_body(&f)).await.json().await.unwrap();
    let key_id = created["id"].as_i64().unwrap();

    let other_id = create_test_user(f.server.db.as_ref(), "other@example.com", "password123").await;
    assert!(other_id > 0);
    let resp = f.client.login("other@example.com", "password123").await;
    let other_token = resp.json::<Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = f
        .client
        .get(&format!("{KEYS}{key_id}/"), Some(&other_token))
        .await;
    assert_eq!(resp.status(), 404, "another user's key is invisible");

    let resp = f
        .client
        .put_json(
            &format!("{KEYS}{key_id}/"),
            Some(&other_token),
            &json!({"name": "hijacked"}),
        )
        .await;
    assert_eq!(resp.status(), 404);

    let resp = f
        .client
        .delete(&format!("{KEYS}{key_id}/"), Some(&other_token))
        .await;
    assert_eq!(resp.status(), 404);

    // The owner still sees it unchanged.
    let resp = f.client.get(KEYS, Some(&f.api_token)).await;
    let listed: Value = resp.json().await.unwrap();
    assert_eq!(listed["keys"][0]["name"], "ci");
}

#[tokio::test]
async fn the_catalog_is_rendered_without_secrets() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .get(&format!("{KEYS}catalog/"), Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);
    let catalog: Value = resp.json().await.unwrap();

    let ids: Vec<&str> = catalog["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"file.write"));
    assert!(ids.contains(&"webdav.read"));
    assert!(
        !ids.iter().any(|id| id.starts_with("key.")),
        "key management must not be grantable"
    );
    assert!(
        !ids.iter().any(|id| id.starts_with("admin.")),
        "admin capabilities are hidden from non-admins"
    );

    let presets: Vec<&str> = catalog["presets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|preset| preset["id"].as_str().unwrap())
        .collect();
    assert!(presets.contains(&"webdav_ro"));
    assert_eq!(catalog["ttl_presets_days"], json!([7, 30, 90, 180, 365]));
    assert_eq!(catalog["max_ttl_days"], 0);
}

#[tokio::test]
async fn the_configured_lifetime_bound_is_enforced() {
    let server = TestServer::start_with_auth_config(|auth| auth.api_key_max_ttl_days = 30).await;
    let client = server.client();
    create_test_user(server.db.as_ref(), "ttl@example.com", "password123").await;
    let resp = client.login("ttl@example.com", "password123").await;
    let token = resp.json::<Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();
    let repo_id = create_test_repo(&client, &token, "ttl-repo").await;

    let body = json!({
        "name": "bounded",
        "capabilities": ["file.read"],
        "all_repos": true,
        "expires_in_days": 30,
    });
    let resp = client.post_json(KEYS, Some(&token), &body).await;
    assert_eq!(resp.status(), 200, "the bound itself is allowed");

    let mut too_long = body.clone();
    too_long["expires_in_days"] = json!(31);
    let resp = client.post_json(KEYS, Some(&token), &too_long).await;
    assert_eq!(resp.status(), 400, "beyond the bound is rejected");

    let mut never = body.clone();
    never.as_object_mut().unwrap().remove("expires_in_days");
    never["never"] = json!(true);
    let resp = client.post_json(KEYS, Some(&token), &never).await;
    assert_eq!(
        resp.status(),
        400,
        "a bound deployment must not allow non-expiring keys"
    );

    // The catalog reports the bound so a UI can hide "never".
    let resp = client.get(&format!("{KEYS}catalog/"), Some(&token)).await;
    let catalog: Value = resp.json().await.unwrap();
    assert_eq!(catalog["max_ttl_days"], 30);
    assert!(repo_id.len() == 36);
}

#[tokio::test]
async fn webdav_capabilities_are_rejected_on_an_encrypted_library() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .create_encrypted_repo_with_password(&f.api_token, "enc", "secret-pw")
        .await;
    assert_eq!(resp.status(), 201);
    let encrypted_repo = resp.json::<Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // WebDAV has no way to supply the library password, so granting it on an
    // encrypted library must be refused at creation.
    let mut body = bound_body(&f);
    body["capabilities"] = json!(["webdav.read"]);
    body["repo_permissions"] = json!([{"repo_id": encrypted_repo, "permission": "rw"}]);
    assert_eq!(create(&f, &body).await.status(), 400);

    // The same key without WebDAV capability is fine.
    let mut body = bound_body(&f);
    body["capabilities"] = json!(["file.read"]);
    body["repo_permissions"] = json!([{"repo_id": encrypted_repo, "permission": "rw"}]);
    assert_eq!(create(&f, &body).await.status(), 200);
}

#[tokio::test]
async fn binding_to_a_library_the_caller_cannot_reach_is_forbidden() {
    let f = TestFixture::new().await;
    create_test_user(f.server.db.as_ref(), "owner@example.com", "password123").await;
    let resp = f.client.login("owner@example.com", "password123").await;
    let other_token = resp.json::<Value>().await.unwrap()["token"]
        .as_str()
        .unwrap()
        .to_string();
    let private_repo = create_test_repo(&f.client, &other_token, "private").await;

    // The caller is not a member, so binding a key to it must fail rather than
    // hand out access the account does not have.
    let mut body = bound_body(&f);
    body["repo_permissions"] = json!([{"repo_id": private_repo, "permission": "rw"}]);
    assert_eq!(create(&f, &body).await.status(), 403);
}

#[tokio::test]
async fn deleting_a_library_removes_keys_bound_only_to_it() {
    let f = TestFixture::new().await;

    // One key bound to the library, one account-wide.
    let bound: Value = create(&f, &bound_body(&f)).await.json().await.unwrap();
    let mut wide = bound_body(&f);
    wide.as_object_mut().unwrap().remove("repo_permissions");
    wide["all_repos"] = json!(true);
    let account_wide: Value = create(&f, &wide).await.json().await.unwrap();
    assert_ne!(bound["id"], account_wide["id"]);

    let resp = f
        .client
        .delete(&format!("/api2/repos/{}/", f.repo_id), Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200, "library deletion failed");

    let listed: Value = f
        .client
        .get(KEYS, Some(&f.api_token))
        .await
        .json()
        .await
        .unwrap();
    let ids: Vec<i64> = listed["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|key| key["id"].as_i64().unwrap())
        .collect();
    assert!(
        !ids.contains(&bound["id"].as_i64().unwrap()),
        "a key with no libraries left is inert and must be removed"
    );
    assert!(
        ids.contains(&account_wide["id"].as_i64().unwrap()),
        "an account-wide key is unaffected"
    );
}
