//! The credential inventory over the JSON API.
//!
//! Two things are worth pinning here. The first is that the response never
//! carries a credential *value*: a `sync_tokens` row holds a ciphertext this
//! server can decrypt, so leaking it would be equivalent to leaking the token
//! the `/api2/repo-tokens/` endpoint deliberately reveals only on request. The
//! second is that the surface is capability-gated like the rest of the device
//! management API, in both directions.

mod common;

use common::TestFixture;
use serde_json::{Value, json};

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

fn key_body(capabilities: &[&str]) -> Value {
    json!({
        "name": "inventory-key",
        "capabilities": capabilities,
        "all_repos": true,
        "repo_permissions": [],
        "expires_in_days": 7,
    })
}

#[tokio::test]
async fn the_inventory_lists_every_kind_without_revealing_values() {
    let f = TestFixture::new().await;

    let resp = f.client.get("/api2/credentials/", Some(&f.api_token)).await;
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    let value: Value = serde_json::from_str(&body).expect("json");

    // The fixture holds a login token (a client session, since the test client
    // reports no device) and one sync token.
    assert!(
        !value["client_sessions"]
            .as_array()
            .expect("client_sessions")
            .is_empty(),
        "the login token is a client session"
    );
    let tokens = value["sync_tokens"].as_array().expect("sync_tokens");
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0]["repo_id"].as_str(), Some(f.repo_id.as_str()));
    assert_eq!(tokens[0]["repo_name"].as_str(), Some("test-repo"));
    assert!(value["browser_sessions"].is_array());
    assert!(value["device_trusts"].is_array());
    assert_eq!(value["api_key_count"].as_u64(), Some(0));

    // The stored row is `enc1:<hex>`; neither the ciphertext nor anything that
    // looks like a token may cross this boundary.
    assert!(
        !body.contains("enc1:"),
        "the inventory must not carry the stored token value"
    );
    assert!(
        !body.contains(&f.sync_token),
        "the inventory must not carry the plaintext token either"
    );
}

/// A device-trust row is listed with its metadata, and revoking it by id works.
#[tokio::test]
async fn a_device_trust_can_be_revoked_through_the_api() {
    let f = TestFixture::new().await;
    let trust = f
        .server
        .repos
        .s2fa_token
        .create_s2fa_token(server::repository::s2fa_token::CreateS2faTokenParams {
            user_id: f.user_id,
            token: server::service::auth::token::generate_api_token(),
            device_id: Some("trusted-device".into()),
            device_name: Some("laptop".into()),
            created_at: 100,
            expires_at: 200,
        })
        .await
        .expect("create trust token");

    let resp = f.client.get("/api2/credentials/", Some(&f.api_token)).await;
    let value: Value = resp.json().await.unwrap();
    assert_eq!(value["device_trusts"][0]["device_id"], "trusted-device");

    let resp = f
        .client
        .delete(
            &format!("/api2/credentials/device_trust/{}/", trust.id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);

    assert!(
        f.server
            .repos
            .s2fa_token
            .list_for_user(f.user_id)
            .await
            .expect("list")
            .is_empty(),
        "the trust is gone"
    );
}

/// An unknown kind is a client error, not a silent no-op: it would otherwise
/// look like a successful revocation.
#[tokio::test]
async fn an_unknown_kind_is_rejected() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .delete("/api2/credentials/session/1/", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 400);
}

/// A real id belonging to another account is a 404, and the row survives.
#[tokio::test]
async fn another_accounts_credential_is_not_reachable() {
    let f = TestFixture::new().await;
    let other =
        common::create_test_user(f.server.db.as_ref(), "other@example.com", "password").await;
    let resp = f.client.create_repo(&f.api_token, "second").await;
    let repo_id = resp.json::<Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    f.server
        .repos
        .sync_token
        .create(&repo_id, other, "peer".to_string(), None, 1, None)
        .await
        .expect("seed");
    let foreign = f
        .server
        .repos
        .sync_token
        .list_for_user(other)
        .await
        .expect("list")[0]
        .id;

    let resp = f
        .client
        .delete(
            &format!("/api2/credentials/sync_token/{foreign}/"),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 404);
    assert_eq!(
        f.server
            .repos
            .sync_token
            .list_for_user(other)
            .await
            .expect("list")
            .len(),
        1
    );
}

/// The surface is gated like the device API it belongs to: reading needs
/// `device.read`, revoking needs `device.write`.
#[tokio::test]
async fn the_inventory_follows_the_device_capabilities() {
    let f = TestFixture::new().await;

    let reader = create_key(&f, &key_body(&["device.read"])).await;
    let resp = f.client.get("/api2/credentials/", Some(&reader)).await;
    assert_eq!(resp.status(), 200, "device.read is enough to list");
    let listed: Value = resp.json().await.unwrap();
    let token_id = listed["sync_tokens"][0]["id"].as_i64().expect("id");

    // Reading is not revoking.
    let resp = f
        .client
        .delete(
            &format!("/api2/credentials/sync_token/{token_id}/"),
            Some(&reader),
        )
        .await;
    assert_eq!(resp.status(), 403, "device.read must not revoke");

    let writer = create_key(&f, &key_body(&["device.write"])).await;
    let resp = f
        .client
        .delete(
            &format!("/api2/credentials/sync_token/{token_id}/"),
            Some(&writer),
        )
        .await;
    assert_eq!(resp.status(), 200, "device.write may revoke");

    // A key with neither is refused outright.
    let unrelated = create_key(&f, &key_body(&["library.read"])).await;
    let resp = f.client.get("/api2/credentials/", Some(&unrelated)).await;
    assert_eq!(resp.status(), 403);
}

/// The legacy device list keeps its narrower answer: the official clients read
/// it, and browser sessions are not devices.
#[tokio::test]
async fn the_device_list_still_answers_only_about_devices() {
    let f = TestFixture::new().await;
    let resp = f.client.get("/api2/devices/", Some(&f.api_token)).await;
    assert_eq!(resp.status(), 200);
    let devices: Value = resp.json().await.unwrap();
    assert!(
        devices.as_array().expect("array").is_empty(),
        "the fixture's login reports no device details, so the legacy list is empty"
    );
}
