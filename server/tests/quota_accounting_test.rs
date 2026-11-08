use crate::common;

use common::{TestFixture, add_repo_member, create_test_user};
use sea_orm::{ActiveModelTrait, EntityTrait, Set};

async fn set_quota(db: &sea_orm::DatabaseConnection, user_id: i32, quota: Option<i64>) {
    let user = infra::entity::user::Entity::find_by_id(user_id)
        .one(db)
        .await
        .unwrap()
        .unwrap();
    let mut active: infra::entity::user::ActiveModel = user.into();
    active.storage_quota = Set(quota);
    active.update(db).await.unwrap();
}

async fn repo_size(f: &TestFixture) -> i64 {
    infra::entity::repo::Entity::find_by_id(f.repo_id.clone())
        .one(&*f.server.db)
        .await
        .unwrap()
        .unwrap()
        .size
}

#[tokio::test]
async fn directory_copy_is_charged() {
    let f = TestFixture::new().await;
    set_quota(&f.server.db, f.user_id, Some(1_000_000)).await;

    let r = f.client.create_dir(&f.api_token, &f.repo_id, "/d").await;
    assert!(r.status().is_success());
    for (name, len) in [("a.txt", 100usize), ("b.txt", 200usize)] {
        let r = f
            .client
            .upload_file(&f.api_token, &f.repo_id, "/d", name, &vec![1u8; len])
            .await;
        assert_eq!(r.status(), 200);
    }
    assert_eq!(repo_size(&f).await, 300);

    let resp = f
        .client
        .batch_copy(&f.api_token, &f.repo_id, "/", &["d"], &f.repo_id, "/")
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let copied = body[0]["obj_name"].as_str().unwrap().to_string();
    assert_eq!(repo_size(&f).await, 600, "directory copy must be charged");

    let resp = f
        .client
        .batch_delete(&f.api_token, &f.repo_id, "/", &[copied.as_str()])
        .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        repo_size(&f).await,
        300,
        "deleting the copy frees only its bytes"
    );
}

#[tokio::test]
async fn concurrent_uploads_do_not_overshoot() {
    let f = TestFixture::new().await;
    set_quota(&f.server.db, f.user_id, Some(1_000)).await;

    let client = &f.client;
    let token = f.api_token.clone();
    let repo = f.repo_id.clone();
    let mut futures = Vec::new();
    for i in 0..8u8 {
        let data = vec![i.wrapping_add(10); 400];
        let name = format!("racer-{i}.bin");
        let token = token.clone();
        let repo = repo.clone();
        futures.push(async move { client.upload_file(&token, &repo, "/", &name, &data).await });
    }
    let results = futures::future::join_all(futures).await;
    let statuses: Vec<_> = results.iter().map(|r| r.status().as_u16()).collect();
    let ok = statuses.iter().filter(|s| **s == 200).count();
    println!(
        "statuses: {statuses:?}; final size: {}",
        repo_size(&f).await
    );
    assert_eq!(ok, 2, "only two 400-byte uploads fit in 1000 bytes");
    assert_eq!(repo_size(&f).await, 800);
}

#[tokio::test]
async fn block_commit_releases_reservations() {
    let f = TestFixture::new().await;
    set_quota(&f.server.db, f.user_id, Some(1_000_000)).await;
    let r = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "seed.txt", b"seed")
        .await;
    assert_eq!(r.status(), 200);

    let url: String = f
        .client
        .upload_blks_link(&f.api_token, &f.repo_id)
        .await
        .json()
        .await
        .unwrap();
    let data = vec![9u8; 100];
    let block_id = infra::crypto::fs_id::sha1_hex(&data);
    let part = reqwest::multipart::Part::bytes(data)
        .file_name(block_id.clone())
        .mime_str("application/octet-stream")
        .unwrap();
    let r = f
        .client
        .post_multipart_url(&url, reqwest::multipart::Form::new().part("file", part))
        .await;
    assert_eq!(r.status(), 200);

    let form = reqwest::multipart::Form::new()
        .text("commitonly", "1")
        .text("parent_dir", "/")
        .text("file_name", "block.bin")
        .text("blockids", serde_json::json!([block_id]).to_string())
        .text("file_size", "100");
    let r = f.client.post_multipart_url(&url, form).await;
    assert_eq!(r.status(), 200);
    assert_eq!(repo_size(&f).await, 104);
    assert_eq!(
        f.server.repos.quota_cache.reserved_total(f.user_id),
        0,
        "commit must release the committed block's reservation"
    );
}

#[tokio::test]
async fn retrying_the_first_chunk_is_idempotent() {
    let f = TestFixture::new().await;
    set_quota(&f.server.db, f.user_id, Some(10_000)).await;
    let url: String = f
        .client
        .get(
            &format!("/api2/repos/{}/upload-link/?p=/", f.repo_id),
            Some(&f.api_token),
        )
        .await
        .json()
        .await
        .unwrap();
    let chunk = vec![1u8; 10];
    for _ in 0..2 {
        let r = f
            .client
            .post_upload_chunk(&url, "/", "big.bin", &chunk, 0, 1_000)
            .await;
        assert_eq!(r.status(), 200);
    }
    assert_eq!(f.server.repos.quota_cache.reserved_total(f.user_id), 1_000);
}

#[tokio::test]
async fn trash_restore_respects_quota() {
    let f = TestFixture::new().await;
    set_quota(&f.server.db, f.user_id, Some(300)).await;
    let r = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "old.bin", &[3u8; 200])
        .await;
    assert_eq!(r.status(), 200);
    let r = f
        .client
        .batch_delete(&f.api_token, &f.repo_id, "/", &["old.bin"])
        .await;
    assert_eq!(r.status(), 200);
    let r = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "new.bin", &vec![4u8; 250])
        .await;
    assert_eq!(r.status(), 200);

    let trash: serde_json::Value = f
        .client
        .get(
            &format!("/api/v2.1/repos/{}/trash/", f.repo_id),
            Some(&f.api_token),
        )
        .await
        .json()
        .await
        .unwrap();
    let item = &trash["items"][0];
    let commit_id = item["commit_id"].as_str().unwrap().to_string();
    let path = format!(
        "{}/{}",
        item["parent_dir"].as_str().unwrap(),
        item["obj_name"].as_str().unwrap()
    );
    let r = f
        .client
        .post_form(
            &format!("/api/v2.1/repos/{}/trash/revert-dirents/", f.repo_id),
            Some(&f.api_token),
            &[("commit_id", &commit_id), ("file_names", &path)],
        )
        .await;
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    println!("restore result: {body}");
    assert_eq!(
        repo_size(&f).await,
        250,
        "over-quota restore must not commit"
    );
    assert_eq!(body["success"].as_array().unwrap().len(), 0);
    assert_eq!(body["failed"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn foreign_member_is_charged_to_owner() {
    let f = TestFixture::new().await;
    let other = create_test_user(&f.server.db, "audit-other@example.com", "password").await;
    add_repo_member(&f.server.db, &f.repo_id, other, "rw").await;
    let login: serde_json::Value = f
        .client
        .login_json("audit-other@example.com", "password")
        .await
        .json()
        .await
        .unwrap();
    let other_token = login["token"].as_str().unwrap().to_string();
    set_quota(&f.server.db, f.user_id, Some(300)).await;
    set_quota(&f.server.db, other, Some(0)).await;
    let r = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "owner.bin", &vec![7u8; 250])
        .await;
    assert_eq!(r.status(), 200);
    let r = f
        .client
        .upload_file(&other_token, &f.repo_id, "/", "member.bin", &[8u8; 200])
        .await;
    println!("member upload status: {}", r.status());
    assert_eq!(
        r.status(),
        443,
        "the owner's quota must bound a member upload"
    );
    assert_eq!(repo_size(&f).await, 250);
}

#[tokio::test]
async fn usage_reconcile_repairs_drift() {
    use server::tasks::run::JobState;
    use server::tasks::spec::JobKey;
    use std::time::{Duration, Instant};

    let f = TestFixture::new().await;
    let r = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "drift.bin", &[5u8; 120])
        .await;
    assert_eq!(r.status(), 200);
    assert_eq!(repo_size(&f).await, 120);

    // Simulate a library whose size drifted before the atomic accounting.
    f.server.repos.repo.set_size(&f.repo_id, 0).await.unwrap();

    let run_id = f
        .server
        .state
        .tasks
        .submit(
            JobKey::UsageReconcile,
            Some(f.user_id),
            serde_json::json!({}),
            "test reconcile",
            None,
        )
        .await
        .expect("the manual job is registered");

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(run) = f.server.state.tasks.store().get(&run_id)
            && run.state.is_terminal()
        {
            assert_eq!(run.state, JobState::Succeeded, "reconcile failed");
            break;
        }
        assert!(Instant::now() < deadline, "reconcile did not finish");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(repo_size(&f).await, 120, "the job must restore the size");
}
