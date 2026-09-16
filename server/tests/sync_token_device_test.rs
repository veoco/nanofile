//! A repository sync token belongs to one device.
//!
//! The credential page groups tokens under the device that holds them, so the
//! server has to know that device. It records it at issuance when the request
//! identifies one, claims an unattributed token for the first device that asks
//! for it, and never reassigns a token that already has an owner — the sync
//! path only refreshes the volatile columns.
//!
//! Both directions matter: a token must appear under its device *before* the
//! first sync (the bug this file pins), and one device's sync must not move
//! another device's token out of its card.

mod common;

use common::TestFixture;
use server::service::user::DeviceService;

/// Log in a further device through `/api2/auth-token/` and return its account
/// token.
async fn login_device(f: &TestFixture, platform: &str, device_id: &str) -> String {
    let resp = f
        .client
        .post_form(
            "/api2/auth-token/",
            None,
            &[
                ("username", "test@example.com"),
                ("password", "password"),
                ("platform", platform),
                ("device_id", device_id),
                ("device_name", "Test Device"),
                ("client_version", "3.0.4"),
            ],
        )
        .await;
    assert_eq!(resp.status(), 200, "device login failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    body["token"].as_str().expect("login token").to_string()
}

/// Ask for one repository's sync token the way a desktop client does.
async fn repo_token(f: &TestFixture, token: &str, repo_id: &str) -> String {
    let resp = f
        .client
        .get(&format!("/api2/repo-tokens/?repos={repo_id}"), Some(token))
        .await;
    assert_eq!(resp.status(), 200);
    let tokens: std::collections::HashMap<String, String> = resp.json().await.unwrap();
    tokens
        .get(repo_id)
        .cloned()
        .unwrap_or_else(|| panic!("no token for {repo_id}: {tokens:?}"))
}

/// Every sync token row the fixture's user holds.
async fn rows(f: &TestFixture) -> Vec<infra::entity::sync_token::Model> {
    f.server
        .repos
        .sync_token
        .list_for_user(f.user_id)
        .await
        .expect("list sync tokens")
}

async fn row_for(f: &TestFixture, repo_id: &str) -> infra::entity::sync_token::Model {
    rows(f)
        .await
        .into_iter()
        .find(|row| row.repo_id == repo_id)
        .unwrap_or_else(|| panic!("no sync token for {repo_id}"))
}

#[tokio::test]
async fn a_token_requested_by_a_device_is_attributed_before_any_sync() {
    let f = TestFixture::new().await;
    let device = login_device(&f, "linux", "dev-a").await;

    // The fixture's token was minted by a device-less login, so it starts
    // unattributed and the device claims it rather than getting a duplicate.
    let token = repo_token(&f, &device, &f.repo_id).await;
    assert_eq!(token, f.sync_token, "the existing token is claimed");

    let row = row_for(&f, &f.repo_id).await;
    assert_eq!(row.peer_id.as_deref(), Some("dev-a"));
    assert_eq!(row.peer_name.as_deref(), Some("Test Device"));
    assert_eq!(row.client_version.as_deref(), Some("3.0.4"));
    assert_eq!(row.last_sync_time, None, "attribution is not a sync");
    assert_eq!(rows(&f).await.len(), 1, "no second row was minted");
}

#[tokio::test]
async fn download_info_hands_a_device_its_own_attributed_token() {
    let f = TestFixture::new().await;
    let device = login_device(&f, "linux", "dev-a").await;

    // A second library whose token was minted without a device identity.
    let resp = f.client.create_repo(&f.api_token, "second").await;
    assert_eq!(resp.status(), 201);
    let second = resp.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .expect("repo id")
        .to_string();
    let orphan = row_for(&f, &second).await;
    assert_eq!(orphan.peer_id, None, "minted by a device-less login");

    let resp = f.client.download_info(&device, &second).await;
    assert_eq!(resp.status(), 200);
    let info: serde_json::Value = resp.json().await.unwrap();

    let row = row_for(&f, &second).await;
    assert_eq!(row.peer_id.as_deref(), Some("dev-a"));
    assert_eq!(row.id, orphan.id, "the same row was claimed, not replaced");
    // The raw value is unchanged, so the client holding it keeps working.
    assert_eq!(
        f.server
            .repos
            .sync_token
            .reveal_token(&row)
            .expect("reveal"),
        info["token"].as_str().unwrap()
    );
}

#[tokio::test]
async fn a_repository_created_by_a_device_has_its_token_from_the_start() {
    let f = TestFixture::new().await;
    let device = login_device(&f, "linux", "dev-a").await;

    let resp = f.client.create_repo(&device, "from-device").await;
    assert_eq!(resp.status(), 201);
    let created = resp.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .expect("repo id")
        .to_string();

    let row = row_for(&f, &created).await;
    assert_eq!(row.peer_id.as_deref(), Some("dev-a"));
    assert_eq!(row.last_sync_time, None);
}

#[tokio::test]
async fn two_devices_hold_their_own_token_and_unlinking_one_keeps_the_other() {
    let f = TestFixture::new().await;
    let first = login_device(&f, "linux", "dev-a").await;
    let second = login_device(&f, "mac", "dev-b").await;

    let token_a = repo_token(&f, &first, &f.repo_id).await;
    let token_b = repo_token(&f, &second, &f.repo_id).await;
    assert_ne!(token_a, token_b, "each device gets its own token");
    assert_eq!(
        token_a, f.sync_token,
        "the first device claims the original"
    );

    let owned = rows(&f).await;
    assert_eq!(owned.len(), 2);
    assert!(
        owned
            .iter()
            .any(|row| row.peer_id.as_deref() == Some("dev-b"))
    );

    // Unlinking one device deletes only what that device holds.
    DeviceService::new(f.server.repos.clone())
        .unlink_device(f.user_id, "linux", "dev-a")
        .await
        .expect("unlink dev-a");

    let left = rows(&f).await;
    assert_eq!(left.len(), 1, "dev-b's token survives: {left:?}");
    assert_eq!(left[0].peer_id.as_deref(), Some("dev-b"));
    assert_eq!(
        f.server.repos.sync_token.reveal_token(&left[0]).as_deref(),
        Some(token_b.as_str())
    );
}

/// The sync path is insert-once: a device that presents another device's token
/// refreshes its timestamp but cannot take the attribution over. A legacy
/// install can still share one token, and the card must not follow whoever
/// synced last.
#[tokio::test]
async fn syncing_never_reassigns_a_token_to_another_device() {
    let f = TestFixture::new().await;
    let device = login_device(&f, "linux", "dev-a").await;
    let token = repo_token(&f, &device, &f.repo_id).await;

    let resp = f
        .client
        .get_sync(
            &format!(
                "/seafhttp/repo/{}/permission-check/?op=download&client_id=dev-b&client_name=Other",
                f.repo_id
            ),
            &token,
        )
        .await;
    assert_eq!(resp.status(), 200);

    let row = row_for(&f, &f.repo_id).await;
    assert_eq!(
        row.peer_id.as_deref(),
        Some("dev-a"),
        "the first device keeps its token"
    );
    assert!(
        row.last_sync_time.is_some(),
        "the sync still records when the token was last used"
    );
}

#[tokio::test]
async fn a_device_less_caller_keeps_the_single_unattributed_token() {
    let f = TestFixture::new().await;

    // `f.api_token` came from a login that reported no device.
    let again = repo_token(&f, &f.api_token, &f.repo_id).await;
    assert_eq!(again, f.sync_token);
    assert_eq!(rows(&f).await.len(), 1, "still one shared row");
    assert_eq!(row_for(&f, &f.repo_id).await.peer_id, None);
}
