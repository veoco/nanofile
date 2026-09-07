#![allow(dead_code)]

mod common;

use common::TestFixture;
use sea_orm::{ActiveModelTrait, EntityTrait, Set};

/// Helper: set a user's storage_quota directly in the database.
async fn set_user_quota(f: &TestFixture, quota: Option<i64>) {
    let user_record = infra::entity::user::Entity::find_by_id(f.user_id)
        .one(&*f.server.db)
        .await
        .unwrap()
        .unwrap();
    let mut active: infra::entity::user::ActiveModel = user_record.into();
    active.storage_quota = Set(quota);
    active.update(&*f.server.db).await.unwrap();
}

#[tokio::test]
async fn test_quota_allows_under_limit() {
    let f = TestFixture::new().await;
    // Set a generous quota: 1 GB.
    set_user_quota(&f, Some(1_073_741_824)).await;

    // Upload a small file — should succeed.
    let data = b"hello world";
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "small.txt", data)
        .await;
    assert_eq!(
        resp.status(),
        200,
        "upload under quota should succeed, got {}",
        resp.status()
    );
}

#[tokio::test]
async fn test_quota_rejects_over_limit() {
    let f = TestFixture::new().await;
    // Set a tiny quota: 1 KB.
    set_user_quota(&f, Some(1024)).await;

    // Upload a file larger than 1 KB — should fail with 443.
    let data = vec![0u8; 2048];
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "large.txt", &data)
        .await;
    assert_eq!(
        resp.status(),
        443,
        "upload over quota should return 443, got {}",
        resp.status()
    );
}

#[tokio::test]
async fn test_quota_unlimited_explicit_zero() {
    let f = TestFixture::new().await;
    // storage_quota = Some(0) means explicitly unlimited.
    set_user_quota(&f, Some(0)).await;

    let data = vec![0u8; 100_000];
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "big.dat", &data)
        .await;
    assert_eq!(
        resp.status(),
        200,
        "explicitly unlimited quota should allow upload, got {}",
        resp.status()
    );
}

#[tokio::test]
async fn test_quota_uses_global_fallback() {
    let f = TestFixture::new().await;
    // storage_quota = None => fall back to global max_storage_bytes
    // (10 GB in the default test config).
    set_user_quota(&f, None).await;

    let data = b"small file within global limit";
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "fallback.txt", data)
        .await;
    assert_eq!(
        resp.status(),
        200,
        "global fallback should allow upload, got {}",
        resp.status()
    );
}

#[tokio::test]
async fn test_sync_quota_check_endpoint_allows() {
    let f = TestFixture::new().await;
    set_user_quota(&f, Some(1_073_741_824)).await; // 1 GB

    let resp = f
        .client
        .get_sync(
            &format!("/seafhttp/repo/{}/quota-check/?delta=1024", f.repo_id),
            &f.sync_token,
        )
        .await;
    assert_eq!(
        resp.status(),
        200,
        "quota-check within limit should return 200, got {}",
        resp.status()
    );
}

#[tokio::test]
async fn test_sync_quota_check_endpoint_rejects() {
    let f = TestFixture::new().await;
    set_user_quota(&f, Some(512)).await; // 512 bytes max

    let resp = f
        .client
        .get_sync(
            &format!("/seafhttp/repo/{}/quota-check/?delta=1024", f.repo_id),
            &f.sync_token,
        )
        .await;
    assert_eq!(
        resp.status(),
        443,
        "quota-check over limit should return 443, got {}",
        resp.status()
    );
}

#[tokio::test]
async fn test_quota_exact_boundary() {
    let f = TestFixture::new().await;
    // Set quota to exactly the file size.
    set_user_quota(&f, Some(11)).await; // "hello world" = 11 bytes

    let data = b"hello world";
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "exact.txt", data)
        .await;
    assert_eq!(
        resp.status(),
        200,
        "upload exactly at quota should succeed (usage=0, delta=quota), got {}",
        resp.status()
    );
}

/// Count block files on disk by walking the two-level prefix directory tree.
fn count_blocks_on_disk(block_dir: &std::path::Path) -> usize {
    let mut count = 0;
    if let Ok(prefix_entries) = std::fs::read_dir(block_dir) {
        for prefix in prefix_entries.flatten() {
            if let Ok(file_entries) = std::fs::read_dir(prefix.path()) {
                count += file_entries.flatten().count();
            }
        }
    }
    count
}

/// Quota failure must clean up newly-written blocks so they don't accumulate
/// as orphan blocks on disk (GC is disabled by default).
#[tokio::test]
async fn test_quota_failure_cleans_up_new_blocks() {
    let f = TestFixture::new().await;
    set_user_quota(&f, Some(1024)).await; // 1 KB quota

    let blocks_before = count_blocks_on_disk(&f.server.block_dir);

    // Upload a file larger than 1 KB — should fail with 443.
    let data = vec![0u8; 2048];
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "large.txt", &data)
        .await;
    assert_eq!(resp.status(), 443, "over-quota upload should be rejected");

    // The newly-written blocks must have been cleaned up — no orphan blocks
    // left on disk.
    let blocks_after = count_blocks_on_disk(&f.server.block_dir);
    assert_eq!(
        blocks_after, blocks_before,
        "orphan blocks must be cleaned up after quota failure (before={}, after={})",
        blocks_before, blocks_after
    );
}

/// Content-Length precheck: an over-quota upload is rejected before any blocks
/// are written to disk.
#[tokio::test]
async fn test_quota_precheck_rejects_without_writing_blocks() {
    let f = TestFixture::new().await;
    set_user_quota(&f, Some(100)).await; // 100 bytes quota

    let blocks_before = count_blocks_on_disk(&f.server.block_dir);

    // Upload a file that's well over the 100-byte quota. The Content-Length
    // header (multipart body) will be even larger than the file, so the
    // precheck should reject before any block I/O.
    let data = vec![0xABu8; 4096];
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "big.dat", &data)
        .await;
    assert_eq!(resp.status(), 443, "precheck should reject over-quota upload");

    let blocks_after = count_blocks_on_disk(&f.server.block_dir);
    assert_eq!(
        blocks_after, blocks_before,
        "no blocks should be written when precheck rejects (before={}, after={})",
        blocks_before, blocks_after
    );
}

/// sync put_block must check quota — an over-quota user can't accumulate
/// unlimited orphan blocks via put_block + never-commit.
#[tokio::test]
async fn test_sync_put_block_rejects_over_quota() {
    let f = TestFixture::new().await;
    set_user_quota(&f, Some(10)).await; // 10 bytes quota

    // Upload one small file to consume the quota.
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "small.txt", b"0123456789")
        .await;
    assert_eq!(resp.status(), 200, "small upload under quota should succeed");

    // Now try to put a block via sync — should be rejected with 443.
    let block_data = b"some block data that exceeds remaining quota";
    let block_id = infra::crypto::fs_id::sha1_hex(block_data);
    let resp = f
        .client
        .put_sync(
            &format!("/seafhttp/repo/{}/block/{}", f.repo_id, block_id),
            &f.sync_token,
            block_data.to_vec(),
        )
        .await;
    assert_eq!(
        resp.status(),
        443,
        "put_block over quota should return 443, got {}",
        resp.status()
    );
}
