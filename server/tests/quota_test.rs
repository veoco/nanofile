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
