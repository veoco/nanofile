//! The sync-protocol permission matrix: which capability every endpoint
//! requires, and how a read-only, removed, or cross-library token behaves.
//!
//! Upstream seafile-server (`fileserver/sync_api.go`) classifies endpoints as
//! `download` (read) or `upload` (write); this matrix pins that mapping. The
//! `check-blocks` row currently records the pre-fix write check and is flipped
//! to 200 by the sync-extractor phase.

use crate::common;

use common::{TestFixture, add_repo_member, create_test_repo, create_test_user, get_sync_token};
use serde_json::Value;

const EMPTY: &str = "0000000000000000000000000000000000000000";

async fn member_sync_token(f: &TestFixture, email: &str, permission: &str) -> (String, i32) {
    let user_id = create_test_user(&f.server.db, email, "password").await;
    add_repo_member(&f.server.db, &f.repo_id, user_id, permission).await;
    let resp = f.client.login(email, "password").await;
    assert_eq!(resp.status(), 200, "member login failed");
    let body: Value = resp.json().await.unwrap();
    let api_token = body["token"].as_str().unwrap().to_string();
    let sync_token = get_sync_token(&f.client, &api_token, &f.repo_id).await;
    (sync_token, user_id)
}

/// Upload a file so head/block endpoints have a real commit to read.
async fn seed(f: &TestFixture) {
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "seed.txt", b"sync seed")
        .await;
    assert!(resp.status().is_success(), "seed upload failed");
}

/// Read endpoints must stay available to a read-only member.
#[tokio::test]
async fn test_readonly_member_read_endpoints_allowed() {
    let f = TestFixture::new().await;
    seed(&f).await;
    let (token, _) = member_sync_token(&f, "reader@test.com", "r").await;

    let resp = f.client.get_head_commit(&token, &f.repo_id).await;
    assert_eq!(resp.status(), 200, "get-head-commit is a read endpoint");

    let resp = f.client.get_commit(&token, &f.repo_id, EMPTY).await;
    assert!(
        resp.status() != 403,
        "get-commit must not require write permission"
    );

    let resp = f.client.fs_id_list(&token, &f.repo_id, EMPTY).await;
    assert_eq!(resp.status(), 200, "fs-id-list is a read endpoint");

    let resp = f.client.pack_fs(&token, &f.repo_id, &[EMPTY]).await;
    assert_eq!(resp.status(), 200, "pack-fs is a read endpoint");

    let resp = f.client.check_fs(&token, &f.repo_id, &[EMPTY]).await;
    assert_eq!(resp.status(), 200, "check-fs is a read endpoint");

    let resp = f.client.get_block(&token, &f.repo_id, EMPTY).await;
    assert!(
        resp.status() != 403,
        "get-block must not require write permission"
    );

    let resp = f.client.block_map(&token, &f.repo_id, EMPTY).await;
    assert!(
        resp.status() != 403,
        "block-map must not require write permission"
    );

    let resp = f
        .client
        .permission_check(&token, &f.repo_id, "download")
        .await;
    assert_eq!(resp.status(), 200);

    let resp = f
        .client
        .get_sync(
            &format!("/seafhttp/repo/{}/quota-check/?delta=0", f.repo_id),
            &token,
        )
        .await;
    assert_eq!(resp.status(), 200, "quota-check is a read endpoint");

    // Upstream classifies check-blocks as a *download* endpoint
    // (`sync_api.go:532`), so a read-only member may probe block existence.
    let resp = f.client.check_blocks(&token, &f.repo_id, &[EMPTY]).await;
    assert_eq!(resp.status(), 200, "check-blocks is a read endpoint");
}

/// Write endpoints must stay forbidden to a read-only member.
#[tokio::test]
async fn test_readonly_member_write_endpoints_forbidden() {
    let f = TestFixture::new().await;
    seed(&f).await;
    let (token, _) = member_sync_token(&f, "reader@test.com", "r").await;

    let resp = f
        .client
        .put_block(&token, &f.repo_id, EMPTY, b"x".to_vec())
        .await;
    assert_eq!(resp.status(), 403, "put-block requires write");

    let resp = f.client.recv_fs(&token, &f.repo_id, b"x".to_vec()).await;
    assert_eq!(resp.status(), 403, "recv-fs requires write");

    let resp = f
        .client
        .put_commit(&token, &f.repo_id, EMPTY, b"{}".to_vec())
        .await;
    assert_eq!(resp.status(), 403, "put-commit requires write");

    let resp = f.client.update_branch(&token, &f.repo_id, EMPTY).await;
    assert_eq!(resp.status(), 403, "update-branch requires write");

    let resp = f
        .client
        .put_sync(
            &format!("/seafhttp/repo/{}/lock-file?p=/seed.txt", f.repo_id),
            &token,
            Vec::new(),
        )
        .await;
    assert_eq!(resp.status(), 403, "lock requires write");

    let resp = f
        .client
        .put_sync(
            &format!("/seafhttp/repo/{}/unlock-file?p=/seed.txt", f.repo_id),
            &token,
            Vec::new(),
        )
        .await;
    assert_eq!(resp.status(), 403, "unlock requires write");

    // permission-check maps a read-only upload probe to a 403 with a reason.
    let resp = f
        .client
        .permission_check(&token, &f.repo_id, "upload")
        .await;
    assert_eq!(resp.status(), 403);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["reason"], "no write permission");
}

/// A member removed after the sync token was issued loses every endpoint.
#[tokio::test]
async fn test_removed_member_is_forbidden() {
    let f = TestFixture::new().await;
    seed(&f).await;
    let (token, user_id) = member_sync_token(&f, "gone@test.com", "rw").await;
    common::remove_repo_member(&f.server.db, &f.repo_id, user_id).await;

    let resp = f.client.get_head_commit(&token, &f.repo_id).await;
    assert_eq!(resp.status(), 403, "removed member must not read commits");

    let resp = f.client.fs_id_list(&token, &f.repo_id, EMPTY).await;
    assert_eq!(
        resp.status(),
        403,
        "removed member must not list fs objects"
    );

    let resp = f.client.get_block(&token, &f.repo_id, EMPTY).await;
    assert_eq!(resp.status(), 403, "removed member must not read blocks");

    let resp = f.client.check_blocks(&token, &f.repo_id, &[EMPTY]).await;
    assert_eq!(resp.status(), 403, "removed member must not probe blocks");

    let resp = f
        .client
        .put_block(&token, &f.repo_id, EMPTY, b"x".to_vec())
        .await;
    assert_eq!(resp.status(), 403);
}

/// A sync token bound to one library must not unlock another.
#[tokio::test]
async fn test_cross_repo_token_is_forbidden() {
    let f = TestFixture::new().await;
    let other = create_test_repo(&f.client, &f.api_token, "other-repo").await;

    let resp = f.client.get_block(&f.sync_token, &other, EMPTY).await;
    assert_eq!(resp.status(), 403, "cross-repo token must be rejected");

    let resp = f.client.fs_id_list(&f.sync_token, &other, EMPTY).await;
    assert_eq!(resp.status(), 403, "cross-repo token must be rejected");
}
