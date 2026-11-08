use crate::common;

use base::common::{CommitData, DirEntryData, FsDirData, FsFileData, S_IFREG};
use common::{TestFixture, fs_object_id};
use sea_orm::{ActiveModelTrait, EntityTrait, Set};

async fn set_quota(f: &TestFixture, quota: Option<i64>) {
    let user = infra::entity::user::Entity::find_by_id(f.user_id)
        .one(&*f.server.db)
        .await
        .unwrap()
        .unwrap();
    let mut active: infra::entity::user::ActiveModel = user.into();
    active.storage_quota = Set(quota);
    active.update(&*f.server.db).await.unwrap();
}

async fn size(f: &TestFixture) -> i64 {
    infra::entity::repo::Entity::find_by_id(f.repo_id.clone())
        .one(&*f.server.db)
        .await
        .unwrap()
        .unwrap()
        .size
}

#[tokio::test]
async fn sync_commit_that_only_adds_a_deduped_file_is_quota_checked() {
    let f = TestFixture::new().await;
    set_quota(&f, Some(60)).await;

    let content = vec![b'a'; 50];
    let c1 = common::sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[("a.txt", content.as_slice(), S_IFREG)],
        &[],
        None,
    )
    .await;
    assert_eq!(size(&f).await, 50);

    // The same content already exists; reference its file object from a second
    // path without uploading a block. That is exactly the dedup case block
    // reservations cannot see.
    let block_id = infra::crypto::fs_id::sha1_hex(&content);
    let file = FsFileData {
        block_ids: vec![block_id],
        size: 50,
        obj_type: 1,
        version: 1,
    };
    let file_id = fs_object_id(serde_json::to_string(&file).unwrap().as_bytes());
    let now = chrono::Utc::now().timestamp();
    let dir = FsDirData {
        dirents: vec![
            DirEntryData {
                id: file_id.clone(),
                mode: S_IFREG,
                modifier: f.email.clone(),
                mtime: now,
                name: "a.txt".into(),
                size: 50,
            },
            DirEntryData {
                id: file_id.clone(),
                mode: S_IFREG,
                modifier: f.email.clone(),
                mtime: now,
                name: "b.txt".into(),
                size: 50,
            },
        ],
        obj_type: 3,
        version: 1,
    };
    let root_json = serde_json::to_string(&dir).unwrap();
    let root_id = fs_object_id(root_json.as_bytes());
    let packed = infra::serialization::pack_fs::compress_fs_data(root_json.as_bytes()).unwrap();
    let mut body = Vec::new();
    body.extend_from_slice(root_id.as_bytes());
    body.extend_from_slice(&(packed.len() as u32).to_be_bytes());
    body.extend_from_slice(&packed);
    assert_eq!(
        f.client
            .recv_fs(&f.sync_token, &f.repo_id, body)
            .await
            .status(),
        200
    );

    let mut commit = CommitData {
        commit_id: String::new(),
        repo_id: f.repo_id.clone(),
        root_id,
        creator_name: f.email.clone(),
        creator: "0".repeat(40),
        description: "dedup copy".into(),
        ctime: now,
        parent_id: Some(c1.clone()),
        second_parent_id: None,
        repo_name: None,
        repo_desc: None,
        repo_category: None,
        encrypted: None,
        enc_version: None,
        magic: None,
        salt: None,
        pwd_hash: None,
        pwd_hash_algo: None,
        pwd_hash_params: None,
        key: None,
        version: 1,
        conflict: None,
        new_merge: None,
    };
    commit.commit_id = server::domain::commit::compute_commit_id(&commit);
    let commit_id = commit.commit_id.clone();
    let body = serde_json::to_string(&commit).unwrap().into_bytes();
    assert_eq!(
        f.client
            .put_commit(&f.sync_token, &f.repo_id, &commit_id, body)
            .await
            .status(),
        200
    );

    let resp = f
        .client
        .update_branch(&f.sync_token, &f.repo_id, &commit_id)
        .await;
    println!("update_branch status: {}", resp.status());
    assert_eq!(
        resp.status(),
        443,
        "logical growth must be charged to the owner"
    );
    assert_eq!(size(&f).await, 50, "a refused commit must not move size");
}
