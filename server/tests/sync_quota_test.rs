use crate::common;

use base::common::{CommitData, DirEntryData, EMPTY_SHA1, FsDirData, FsFileData, S_IFREG};
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

/// `size` for a library other than the fixture's own (an encrypted one, say).
async fn repo_size(f: &TestFixture, repo_id: &str) -> i64 {
    infra::entity::repo::Entity::find_by_id(repo_id.to_string())
        .one(&*f.server.db)
        .await
        .unwrap()
        .unwrap()
        .size
}

/// The usage the client is shown (`GET /api2/account/info/`), i.e. what quota is
/// enforced against.
async fn account_usage(f: &TestFixture) -> i64 {
    let resp = f
        .client
        .get("/api2/account/info/", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200, "account info failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    body["usage"].as_i64().unwrap()
}

async fn head_of(f: &TestFixture, sync_token: &str, repo_id: &str) -> String {
    let resp = f.client.get_head_commit(sync_token, repo_id).await;
    assert_eq!(resp.status(), 200, "get_head_commit failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    body["head_commit_id"].as_str().unwrap().to_string()
}

/// Upload one block under its content-addressed id, as a sync client does.
async fn put_block(f: &TestFixture, sync_token: &str, repo_id: &str, data: &[u8]) -> String {
    let id = infra::crypto::fs_id::sha1_hex(data);
    let resp = f
        .client
        .put_block(sync_token, repo_id, &id, data.to_vec())
        .await;
    assert_eq!(resp.status(), 200, "put_block failed");
    id
}

/// Upload one FS object (file or directory JSON) under its content-addressed id.
async fn put_fs_object(f: &TestFixture, sync_token: &str, repo_id: &str, json: &str) -> String {
    let id = fs_object_id(json.as_bytes());
    let packed = infra::serialization::pack_fs::compress_fs_data(json.as_bytes()).unwrap();
    let mut body = Vec::new();
    body.extend_from_slice(id.as_bytes());
    body.extend_from_slice(&(packed.len() as u32).to_be_bytes());
    body.extend_from_slice(&packed);
    let resp = f.client.recv_fs(sync_token, repo_id, body).await;
    if !resp.status().is_success() {
        let status = resp.status();
        panic!(
            "recv_fs for an FS object failed: {status} {}",
            resp.text().await.unwrap_or_default()
        );
    }
    id
}

/// Upload `root` as a commit for `repo_id` and attempt the branch update. Returns the
/// commit id and the update-branch response (asserted 200 only for `put_commit`), so a
/// rejection test can inspect the status.
async fn attempt_root_commit(
    f: &TestFixture,
    sync_token: &str,
    repo_id: &str,
    parent: Option<&str>,
    root: &FsDirData,
) -> (String, reqwest::Response) {
    use std::sync::atomic::{AtomicU64, Ordering};

    static COMMIT_SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = COMMIT_SEQ.fetch_add(1, Ordering::Relaxed);

    let root_id = put_fs_object(
        f,
        sync_token,
        repo_id,
        &serde_json::to_string(root).unwrap(),
    )
    .await;
    let now = chrono::Utc::now().timestamp();
    let mut commit = CommitData {
        commit_id: String::new(),
        repo_id: repo_id.to_string(),
        root_id,
        creator_name: f.email.clone(),
        creator: EMPTY_SHA1.to_string(),
        description: format!("size invariant {now}-{seq}"),
        ctime: now,
        parent_id: parent.map(str::to_string),
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
    let resp = f
        .client
        .put_commit(sync_token, repo_id, &commit_id, body)
        .await;
    assert_eq!(resp.status(), 200, "put_commit failed");
    let resp = f
        .client
        .update_branch(sync_token, repo_id, &commit_id)
        .await;
    (commit_id, resp)
}

/// Upload `blocks` and a file object declaring `file_size`, wrap it in a root whose
/// dirent claims `dirent_size`, and attempt the commit. The two sizes are independent
/// on purpose: the tests below forge the very inconsistencies the invariant rejects.
#[allow(clippy::too_many_arguments)]
async fn attempt_file_commit(
    f: &TestFixture,
    sync_token: &str,
    repo_id: &str,
    parent: Option<&str>,
    name: &str,
    blocks: &[&[u8]],
    file_size: i64,
    dirent_size: i64,
) -> (String, reqwest::Response) {
    let mut block_ids = Vec::new();
    for block in blocks {
        block_ids.push(put_block(f, sync_token, repo_id, block).await);
    }
    let file = FsFileData {
        block_ids,
        size: file_size,
        obj_type: 1,
        version: 1,
    };
    let file_id = put_fs_object(
        f,
        sync_token,
        repo_id,
        &serde_json::to_string(&file).unwrap(),
    )
    .await;
    let root = FsDirData {
        dirents: vec![DirEntryData {
            id: file_id,
            mode: S_IFREG,
            modifier: f.email.clone(),
            mtime: chrono::Utc::now().timestamp(),
            name: name.to_string(),
            size: dirent_size,
        }],
        obj_type: 3,
        version: 1,
    };
    attempt_root_commit(f, sync_token, repo_id, parent, &root).await
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

/// A first upload cannot under-report a regular file's bytes. `full_check_blocks`
/// must compare the declared size with the blocks actually stored: a file object
/// that references 64 bytes while declaring 8 (or 0) would otherwise store 64 and
/// charge 8. A negative dirent size and the zero-byte `EMPTY_SHA1` sentinel claiming
/// bytes are malformed trees. None of these may move `repo.size`, the user's usage
/// or HEAD.
#[tokio::test]
async fn first_upload_with_inconsistent_file_size_is_rejected() {
    let f = TestFixture::new().await;
    let payload = vec![b'x'; 64];

    // The file object says 8 bytes but its single block holds 64.
    let (_, resp) = attempt_file_commit(
        &f,
        &f.sync_token,
        &f.repo_id,
        None,
        "understated.bin",
        &[payload.as_slice()],
        8,
        8,
    )
    .await;
    assert_eq!(
        resp.status(),
        400,
        "an understated file size must be rejected"
    );

    // A file object referencing a block while declaring zero bytes.
    let (_, resp) = attempt_file_commit(
        &f,
        &f.sync_token,
        &f.repo_id,
        None,
        "zero.bin",
        &[payload.as_slice()],
        0,
        0,
    )
    .await;
    assert_eq!(
        resp.status(),
        400,
        "size 0 with a non-empty block must be rejected"
    );

    // A negative dirent size must not reach the accounting.
    let (_, resp) = attempt_file_commit(
        &f,
        &f.sync_token,
        &f.repo_id,
        None,
        "negative.bin",
        &[payload.as_slice()],
        64,
        -1,
    )
    .await;
    assert_eq!(
        resp.status(),
        400,
        "a negative dirent size must be rejected"
    );

    // The `EMPTY_SHA1` sentinel stands for a zero-byte file: no object, no bytes.
    let now = chrono::Utc::now().timestamp();
    let empty_claim = FsDirData {
        dirents: vec![DirEntryData {
            id: EMPTY_SHA1.to_string(),
            mode: S_IFREG,
            modifier: f.email.clone(),
            mtime: now,
            name: "empty.bin".to_string(),
            size: 5,
        }],
        obj_type: 3,
        version: 1,
    };
    let (_, resp) = attempt_root_commit(&f, &f.sync_token, &f.repo_id, None, &empty_claim).await;
    assert_eq!(
        resp.status(),
        400,
        "the empty sentinel must not declare bytes"
    );

    assert_eq!(
        size(&f).await,
        0,
        "rejected commits must not move repo.size"
    );
    assert_eq!(
        account_usage(&f).await,
        0,
        "rejected commits must not be charged"
    );
    assert_eq!(
        head_of(&f, &f.sync_token, &f.repo_id).await,
        EMPTY_SHA1,
        "rejected commits must not move HEAD"
    );

    // A consistent upload of the same content still lands and is charged in full.
    common::sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[("ok.bin", payload.as_slice(), S_IFREG)],
        &[],
        None,
    )
    .await;
    assert_eq!(
        size(&f).await,
        64,
        "the valid upload must be charged in full"
    );
    assert_eq!(account_usage(&f).await, 64);
}

/// On the incremental path (`check_commit_blocks`' two-sided diff) a changed file's
/// dirent and file object must agree, and the empty sentinel still cannot claim
/// bytes. Every attempt is a 400 that leaves the previous commit's size untouched.
#[tokio::test]
async fn incremental_commit_with_inconsistent_file_size_is_rejected() {
    let f = TestFixture::new().await;
    let base = common::sync_commit(
        &f.client,
        &f.sync_token,
        &f.repo_id,
        &[("a.txt", vec![b'a'; 50].as_slice(), S_IFREG)],
        &[],
        None,
    )
    .await;
    assert_eq!(size(&f).await, 50);
    let usage_before = account_usage(&f).await;

    // The changed file object says 50 bytes, its dirent claims 5: accepting this
    // would persist a tree that under-reports the file.
    let content = vec![b'b'; 50];
    let (_, resp) = attempt_file_commit(
        &f,
        &f.sync_token,
        &f.repo_id,
        Some(&base),
        "a.txt",
        &[content.as_slice()],
        50,
        5,
    )
    .await;
    assert_eq!(
        resp.status(),
        400,
        "a dirent/file-object size mismatch must be rejected"
    );

    // Same change with a negative dirent size.
    let (_, resp) = attempt_file_commit(
        &f,
        &f.sync_token,
        &f.repo_id,
        Some(&base),
        "a.txt",
        &[content.as_slice()],
        50,
        -7,
    )
    .await;
    assert_eq!(
        resp.status(),
        400,
        "a negative dirent size must be rejected"
    );

    // The zero-byte sentinel claiming bytes is malformed on this path too.
    let now = chrono::Utc::now().timestamp();
    let empty_claim = FsDirData {
        dirents: vec![DirEntryData {
            id: EMPTY_SHA1.to_string(),
            mode: S_IFREG,
            modifier: f.email.clone(),
            mtime: now,
            name: "empty.bin".to_string(),
            size: 5,
        }],
        obj_type: 3,
        version: 1,
    };
    let (_, resp) =
        attempt_root_commit(&f, &f.sync_token, &f.repo_id, Some(&base), &empty_claim).await;
    assert_eq!(
        resp.status(),
        400,
        "the empty sentinel must not declare bytes"
    );

    assert_eq!(
        size(&f).await,
        50,
        "rejected commits must not move repo.size"
    );
    assert_eq!(
        account_usage(&f).await,
        usage_before,
        "rejected commits must not be charged"
    );
    assert_eq!(
        head_of(&f, &f.sync_token, &f.repo_id).await,
        base,
        "rejected commits must not move HEAD"
    );
}

/// A client-side-encrypted library stores PKCS7 ciphertext blocks, so a block may be
/// up to 16 bytes longer than the plaintext it holds. The bound is per block: exactly
/// 16 bytes of padding is accepted, one byte more (and a stored total *below* the
/// declared plaintext) is a 400 that is never charged.
#[tokio::test]
async fn encrypted_library_accepts_pkcs7_padding_up_to_the_bound() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .create_encrypted_repo_with_password(&f.api_token, "enc-size", "secret-pw")
        .await;
    assert_eq!(resp.status(), 200, "encrypted library creation failed");
    let body: serde_json::Value = resp.json().await.unwrap();
    let repo_id = body["id"].as_str().unwrap().to_string();
    let sync_token = common::get_sync_token(&f.client, &f.api_token, &repo_id).await;

    // 16 plaintext bytes encode to 32 ciphertext bytes: PKCS7 adds exactly 16.
    let boundary = vec![0u8; 32];
    let (head1, resp) = attempt_file_commit(
        &f,
        &sync_token,
        &repo_id,
        None,
        "enc.bin",
        &[boundary.as_slice()],
        16,
        16,
    )
    .await;
    assert_eq!(
        resp.status(),
        200,
        "padding at the per-block bound must be accepted"
    );
    assert_eq!(repo_size(&f, &repo_id).await, 16);
    assert_eq!(account_usage(&f).await, 16);

    // 17 bytes of padding cannot come from any PKCS7 ciphertext.
    let too_long = vec![0u8; 33];
    let (_, resp) = attempt_file_commit(
        &f,
        &sync_token,
        &repo_id,
        Some(&head1),
        "enc.bin",
        &[too_long.as_slice()],
        16,
        16,
    )
    .await;
    assert_eq!(resp.status(), 400, "oversized padding must be rejected");

    // Fewer stored bytes than the declared plaintext is impossible as well.
    let (_, resp) = attempt_file_commit(
        &f,
        &sync_token,
        &repo_id,
        Some(&head1),
        "enc.bin",
        &[boundary.as_slice()],
        40,
        40,
    )
    .await;
    assert_eq!(
        resp.status(),
        400,
        "a stored total below the declared size must be rejected"
    );

    assert_eq!(
        repo_size(&f, &repo_id).await,
        16,
        "rejected commits must not move repo.size"
    );
    assert_eq!(
        account_usage(&f).await,
        16,
        "rejected commits must not be charged"
    );
    assert_eq!(
        head_of(&f, &sync_token, &repo_id).await,
        head1,
        "rejected commits must not move HEAD"
    );
}
