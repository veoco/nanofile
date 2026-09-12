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

/// Count block files on disk.
///
/// Blocks are stored per library:
/// `{block_dir}/repos/<sha1(repo_id)>/<2hex>/<block_id>`. The `.repo_id` marker
/// lives directly in the library directory, so only the two-hex-digit prefix
/// directories are descended into.
fn count_blocks_on_disk(block_dir: &std::path::Path) -> usize {
    let mut count = 0;
    let Ok(repos) = std::fs::read_dir(block_dir.join("repos")) else {
        return 0;
    };
    for repo_dir in repos.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
        let Ok(prefixes) = std::fs::read_dir(&repo_dir) else {
            continue;
        };
        for prefix in prefixes.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
            if let Ok(files) = std::fs::read_dir(&prefix) {
                count += files.flatten().filter(|f| f.path().is_file()).count();
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
    assert_eq!(
        resp.status(),
        443,
        "precheck should reject over-quota upload"
    );

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
    assert_eq!(
        resp.status(),
        200,
        "small upload under quota should succeed"
    );

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

/// Regression: a block write does not change any repo's `size`, so checking
/// committed usage alone let `put_block` succeed forever. Distinct blocks that
/// are never committed must each consume quota until they are committed — that
/// is what bounds "write blocks, never commit".
#[tokio::test]
async fn uncommitted_blocks_accumulate_against_quota() {
    let f = TestFixture::new().await;
    set_user_quota(&f, Some(1_000)).await; // 1 KB budget

    // Two distinct blocks that are each comfortably inside the budget but
    // together exceed it: the first must be accepted, the second rejected.
    let first = vec![7u8; 700];
    let second = vec![9u8; 700];

    for (idx, data) in [&first, &second].into_iter().enumerate() {
        let block_id = infra::crypto::fs_id::sha1_hex(data);
        let resp = f
            .client
            .put_sync(
                &format!("/seafhttp/repo/{}/block/{}", f.repo_id, block_id),
                &f.sync_token,
                data.clone(),
            )
            .await;
        let expected = if idx == 0 { 200 } else { 443 };
        assert_eq!(
            resp.status(),
            expected,
            "block #{idx}: uncommitted writes must accumulate against the quota \
             (got {}, expected {expected})",
            resp.status()
        );
    }

    assert_eq!(
        count_blocks_on_disk(&f.server.block_dir),
        1,
        "only the accepted block may be on disk"
    );
}

/// The reservation is not a permanent tax: committing a file moves its bytes
/// into the repo's committed `size`, so the reservation for that repo is
/// released and the remaining budget stays usable.
#[tokio::test]
async fn committing_a_file_releases_its_block_reservation() {
    let f = TestFixture::new().await;
    set_user_quota(&f, Some(2_000)).await;

    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "a.bin", &vec![3u8; 900])
        .await;
    assert_eq!(resp.status(), 200, "first upload failed");

    // A second upload of the same size must fit: if the first upload's
    // reservation were still held, its bytes would be counted twice.
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "b.bin", &vec![4u8; 900])
        .await;
    assert_eq!(
        resp.status(),
        200,
        "the committed file must not still hold an uncommitted reservation, got: {}",
        resp.status()
    );

    // The budget is now genuinely full.
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "c.bin", &vec![5u8; 400])
        .await;
    assert_eq!(resp.status(), 443, "quota must still apply after commits");
}

/// A resumable upload reserves the size it declares in `Content-Range` on its
/// first chunk. Without that, an abandoned upload left its blocks on disk while
/// every later upload still saw a full quota.
#[tokio::test]
async fn resumable_upload_reserves_its_declared_size() {
    let f = TestFixture::new().await;
    set_user_quota(&f, Some(1_000)).await;

    let token_resp = f
        .client
        .get(
            &format!("/api2/repos/{}/upload-link/?p=/", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(token_resp.status(), 200, "upload-link failed");
    let upload_url: String = token_resp.json().await.unwrap();

    // Declare 512 KB, send only ten bytes, and never send the rest.
    let declared_total = 512 * 1024;
    let resp = f
        .client
        .post_upload_chunk(&upload_url, "/", "big.bin", &[1u8; 10], 0, declared_total)
        .await;
    assert_eq!(
        resp.status(),
        443,
        "a declared size beyond the quota must be rejected before anything is \
         written, got: {}",
        resp.status()
    );
    assert_eq!(
        count_blocks_on_disk(&f.server.block_dir),
        0,
        "nothing may be written when the reservation fails"
    );
}

/// Cleaning up an abandoned upload must never delete a block that a committed
/// file references. An upload of identical content can see a block as
/// pre-existing (`was_new == false`), so it is not in the abandoned upload's
/// cleanup set — yet it can be the very block another client just committed.
/// The reaper therefore re-checks every candidate against the FS objects.
#[tokio::test]
async fn committed_blocks_are_never_reported_as_orphans() {
    let f = TestFixture::new().await;

    let content = b"content that is already committed".to_vec();
    f.client
        .upload_file(&f.api_token, &f.repo_id, "/", "kept.txt", &content)
        .await;

    // A small file is a single block whose id is the SHA-1 of its bytes, so the
    // test can name the block a committed file depends on.
    let block_id = infra::crypto::fs_id::sha1_hex(&content);
    assert!(
        f.server
            .repos
            .fs_object
            .references_block(&f.repo_id, &block_id)
            .await
            .unwrap(),
        "a block a committed file references must be reported as referenced"
    );

    // Anything that is not a content id must fail safe (keep the block) rather
    // than authorize a delete; a real but unreferenced id reports false.
    for malformed in ["", "%", "_", "not-hex", &"z".repeat(40)] {
        assert!(
            f.server
                .repos
                .fs_object
                .references_block(&f.repo_id, malformed)
                .await
                .unwrap(),
            "malformed block id {malformed:?} must never be deletable"
        );
    }
    assert!(
        !f.server
            .repos
            .fs_object
            .references_block(&f.repo_id, &"a".repeat(40))
            .await
            .unwrap(),
        "an unreferenced content id must be reported as unreferenced"
    );
}
