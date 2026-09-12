//! Enumeration and revocation of the credentials an account holds.
//!
//! The credential inventory can only be useful if it can *see* everything the
//! account holds — so these tests pin both halves: that enumeration does not
//! silently drop rows, and that every revoke is scoped to the owner, because
//! these ids are exposed to a page and a guessed id must not reach another
//! account.

mod common;

use common::{TestFixture, create_test_user};
use sea_orm::{ActiveModelTrait, Set};

/// A device-trust token for `user_id`, so the inventory has one to find.
async fn trust_token(f: &TestFixture, user_id: i32, device_id: &str) -> i32 {
    f.server
        .repos
        .s2fa_token
        .create_s2fa_token(server::repository::s2fa_token::CreateS2faTokenParams {
            user_id,
            token: server::service::auth::token::generate_api_token(),
            device_id: Some(device_id.to_string()),
            device_name: Some("laptop".to_string()),
            created_at: 100,
            expires_at: i64::MAX,
        })
        .await
        .expect("create trust token")
        .id
}

/// A second session for `user_id`, inserted directly so the test controls its
/// `platform` and `is_pending` flags.
async fn raw_session(
    f: &TestFixture,
    user_id: i32,
    platform: Option<&str>,
    is_pending: bool,
) -> i32 {
    let raw = server::service::auth::token::generate_api_token();
    let model = infra::entity::api_token::ActiveModel {
        id: sea_orm::NotSet,
        user_id: Set(user_id),
        token: Set(server::service::auth::token::hash_token(&raw)),
        created_at: Set(1_000),
        expires_at: Set(None),
        device_id: Set(platform.map(|_| "dev".to_string())),
        platform: Set(platform.map(str::to_string)),
        device_name: Set(None),
        client_version: Set(None),
        is_pending: Set(is_pending),
        source: Set("client".to_string()),
        user_agent: Set(None),
    };
    model
        .insert(f.server.db.as_ref())
        .await
        .expect("insert session")
        .id
}

/// A session that is not the fixture's login and carries no device details is
/// still a session: filtering on `platform` used to hide it.
#[tokio::test]
async fn sessions_are_listed_without_a_platform() {
    let f = TestFixture::new().await;
    let extra = raw_session(&f, f.user_id, None, false).await;

    let sessions = f
        .server
        .repos
        .api_token
        .list_sessions(f.user_id)
        .await
        .expect("list");
    let ids: Vec<i32> = sessions.iter().map(|s| s.id).collect();
    assert!(
        ids.contains(&extra),
        "a session without a platform is listed"
    );
    assert!(
        sessions.len() >= 2,
        "the login token and the extra session are both listed"
    );
}

/// A pending 2FA token is a five-minute half-credential: listing it would offer
/// the owner a "session" they never completed.
#[tokio::test]
async fn pending_tokens_are_never_listed() {
    let f = TestFixture::new().await;
    let pending = raw_session(&f, f.user_id, None, true).await;

    let ids: Vec<i32> = f
        .server
        .repos
        .api_token
        .list_sessions(f.user_id)
        .await
        .expect("list")
        .iter()
        .map(|s| s.id)
        .collect();
    assert!(
        !ids.contains(&pending),
        "a pending token must not be listed"
    );
    assert!(
        f.server
            .repos
            .api_token
            .find_by_id_and_user(pending, f.user_id)
            .await
            .expect("find")
            .is_some(),
        "it is still addressable directly, which is what the 2FA flow needs"
    );
}

/// A sync token is enumerated with its metadata and nothing else — the stored
/// value is a bearer credential, so the inventory must never carry it.
#[tokio::test]
async fn sync_tokens_are_enumerated_for_their_owner() {
    let f = TestFixture::new().await;
    let listed = f
        .server
        .repos
        .sync_token
        .list_for_user(f.user_id)
        .await
        .expect("list");

    assert_eq!(listed.len(), 1, "the fixture holds one sync token");
    assert_eq!(listed[0].repo_id, f.repo_id);
    assert!(
        listed[0]
            .token
            .starts_with(infra::crypto::token_encryption::TOKEN_CIPHER_PREFIX),
        "the row carries the ciphertext, which is why callers must not \
         serialise the model directly"
    );
}

/// Device trusts belong to the account that created them.
#[tokio::test]
async fn device_trusts_are_enumerated_for_their_owner() {
    let f = TestFixture::new().await;
    trust_token(&f, f.user_id, "device-a").await;
    let other_user = create_test_user(f.server.db.as_ref(), "other@example.com", "password").await;
    trust_token(&f, other_user, "device-b").await;

    let mine = f
        .server
        .repos
        .s2fa_token
        .list_for_user(f.user_id)
        .await
        .expect("list");
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].device_id.as_deref(), Some("device-a"));
}

/// Every revoke takes the owner as well as the id. These ids travel through a
/// page, so an id alone must never be enough to delete someone else's row.
#[tokio::test]
async fn revocation_is_scoped_to_the_owner() {
    let f = TestFixture::new().await;
    let other_user = create_test_user(f.server.db.as_ref(), "other@example.com", "password").await;

    let session = raw_session(&f, other_user, None, false).await;
    let trust = trust_token(&f, other_user, "device-b").await;
    let sync = f
        .server
        .repos
        .sync_token
        .list_for_user(f.user_id)
        .await
        .expect("list")[0]
        .id;

    // The ids are real, but they belong to somebody else.
    assert_eq!(
        f.server
            .repos
            .api_token
            .delete_by_id_and_user(session, f.user_id)
            .await
            .expect("delete"),
        0,
        "another account's session must not be revoked"
    );
    assert_eq!(
        f.server
            .repos
            .s2fa_token
            .delete_by_id_and_user(trust, f.user_id)
            .await
            .expect("delete"),
        0,
        "another account's device trust must not be revoked"
    );
    assert_eq!(
        f.server
            .repos
            .sync_token
            .delete_by_id_and_user(sync, other_user)
            .await
            .expect("delete"),
        0,
        "another account's sync token must not be revoked"
    );

    // All three rows survive, and the owner can still revoke their own.
    assert!(
        f.server
            .repos
            .api_token
            .find_by_id_and_user(session, other_user)
            .await
            .expect("find")
            .is_some()
    );
    assert_eq!(
        f.server
            .repos
            .sync_token
            .delete_by_id_and_user(sync, f.user_id)
            .await
            .expect("delete"),
        1
    );
    assert_eq!(
        f.server
            .repos
            .s2fa_token
            .delete_by_id_and_user(trust, other_user)
            .await
            .expect("delete"),
        1
    );
}
