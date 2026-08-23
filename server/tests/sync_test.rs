mod common;

use common::{TestFixture, TestServer, create_test_user, get_sync_token};
use sea_orm::{ActiveModelTrait, EntityTrait};

#[tokio::test]
async fn test_protocol_version() {
    let server = TestServer::start().await;
    let client = server.client();

    let resp = client.protocol_version().await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["version"].as_i64().unwrap(), 2);
}

#[tokio::test]
async fn test_permission_check_upload() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, token, "My Library").await;
    let sync_token = get_sync_token(&client, token, &repo_id).await;

    let resp = client
        .permission_check(&sync_token, &repo_id, "upload")
        .await;
    assert_eq!(resp.status(), 200);
    // seafile-server returns empty body on success (HTTP 200, no content)
    let body_bytes = resp.bytes().await.unwrap();
    assert!(body_bytes.is_empty());
}

#[tokio::test]
async fn test_permission_check_download() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, token, "My Library").await;
    let sync_token = get_sync_token(&client, token, &repo_id).await;

    let resp = client
        .permission_check(&sync_token, &repo_id, "download")
        .await;
    assert_eq!(resp.status(), 200);
    // seafile-server returns empty body on success
    let body_bytes = resp.bytes().await.unwrap();
    assert!(body_bytes.is_empty());
}

#[tokio::test]
async fn test_get_head_commit_no_commits() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, token, "My Library").await;
    let sync_token = get_sync_token(&client, token, &repo_id).await;

    let resp = client.get_head_commit(&sync_token, &repo_id).await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["is_corrupted"].as_i64().unwrap(), 0);
    assert_eq!(
        body["head_commit_id"].as_str().unwrap(),
        "0000000000000000000000000000000000000000"
    );
}

#[tokio::test]
async fn test_head_commits_multi_no_updates() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, token, "My Library").await;

    let resp = client.head_commits_multi(&[&repo_id]).await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    // Empty repo with no commits should not return anything
    let obj = body.as_object().unwrap();
    assert!(obj.is_empty());
}

#[tokio::test]
async fn test_head_commits_multi_with_updates() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, token, "My Library").await;

    // Create a commit manually in the DB
    let commit_id = "a".repeat(40);
    let root_id = "b".repeat(40);
    let now = chrono::Utc::now().timestamp();
    let commit = infra::entity::commit::ActiveModel {
        id: sea_orm::NotSet,
        repo_id: sea_orm::Set(repo_id.clone()),
        commit_id: sea_orm::Set(commit_id.clone()),
        root_id: sea_orm::Set(root_id),
        parent_id: sea_orm::NotSet,
        second_parent_id: sea_orm::NotSet,
        creator_name: sea_orm::Set("test@example.com".to_string()),
        creator: sea_orm::Set(base::common::EMPTY_SHA1.to_string()),
        description: sea_orm::Set("test commit".to_string()),
        ctime: sea_orm::Set(now),
        version: sea_orm::Set(1),
    };
    commit.insert(server.db.as_ref()).await.unwrap();

    // Update repo HEAD
    let mut repo: infra::entity::repo::ActiveModel =
        infra::entity::repo::Entity::find_by_id(&repo_id)
            .one(server.db.as_ref())
            .await
            .unwrap()
            .unwrap()
            .into();
    repo.head_commit_id = sea_orm::Set(Some(commit_id.clone()));
    repo.update(server.db.as_ref()).await.unwrap();

    let resp = client.head_commits_multi(&[&repo_id]).await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body[&repo_id].as_str().unwrap(), commit_id);
}

#[tokio::test]
async fn test_fs_id_list_empty_repo() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, token, "My Library").await;
    let sync_token = get_sync_token(&client, token, &repo_id).await;

    let resp = client
        .fs_id_list(
            &sync_token,
            &repo_id,
            "0000000000000000000000000000000000000000",
        )
        .await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let arr = body.as_array().unwrap();
    assert!(arr.is_empty());
}

#[tokio::test]
async fn test_check_fs_all_missing() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, token, "My Library").await;
    let sync_token = get_sync_token(&client, token, &repo_id).await;

    let resp = client
        .check_fs(&sync_token, &repo_id, &["fs_id_1", "fs_id_2"])
        .await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let missing = body.as_array().unwrap();
    assert_eq!(missing.len(), 2);
}

#[tokio::test]
async fn test_check_blocks_all_missing() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, token, "My Library").await;
    let sync_token = get_sync_token(&client, token, &repo_id).await;

    // Valid-format (40-hex) but non-existent block ids are reported missing.
    let resp = client
        .check_blocks(
            &sync_token,
            &repo_id,
            &[
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            ],
        )
        .await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let missing = body.as_array().unwrap();
    assert_eq!(missing.len(), 2);
}

#[tokio::test]
async fn test_put_and_get_block() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, token, "My Library").await;
    let sync_token = get_sync_token(&client, token, &repo_id).await;

    let block_data = b"hello world block data";
    let block_id = {
        use sha1::{Digest, Sha1};
        let mut hasher = Sha1::new();
        hasher.update(block_data);
        hex::encode(hasher.finalize())
    };

    let resp = client
        .put_block(&sync_token, &repo_id, &block_id, block_data.to_vec())
        .await;
    assert_eq!(resp.status(), 200);

    let resp = client.get_block(&sync_token, &repo_id, &block_id).await;
    assert_eq!(resp.status(), 200);

    let data = resp.bytes().await.unwrap();
    assert_eq!(data.as_ref(), block_data);
}

// ==================== Security: oversized bodies ====================

/// Security: put_block must reject bodies larger than the per-block cap
/// (8 MiB) instead of buffering up to the 4 GiB upload limit.
#[tokio::test]
async fn test_put_block_rejects_oversized_body() {
    let f = TestFixture::new().await;

    // Block ids must pass validate_block_id (40 lowercase hex/digits).
    let block_id = "0".repeat(40);
    let resp = f
        .client
        .put_block(
            &f.sync_token,
            &f.repo_id,
            &block_id,
            vec![0u8; 8 * 1024 * 1024 + 1],
        )
        .await;
    assert_eq!(resp.status(), 500);
}

/// Security: recv_fs must reject packs larger than the 32 MiB cap.
#[tokio::test]
async fn test_recv_fs_rejects_oversized_body() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .recv_fs(&f.sync_token, &f.repo_id, vec![0u8; 32 * 1024 * 1024 + 1])
        .await;
    assert_eq!(resp.status(), 500);
}

/// Security: put_commit must reject bodies larger than the 4 MiB cap.
#[tokio::test]
async fn test_put_commit_rejects_oversized_body() {
    let f = TestFixture::new().await;

    let commit_id = "0".repeat(40);
    let resp = f
        .client
        .put_commit(
            &f.sync_token,
            &f.repo_id,
            &commit_id,
            vec![0u8; 4 * 1024 * 1024 + 1],
        )
        .await;
    assert_eq!(resp.status(), 500);
}

// ==================== Regression tests ====================

/// Regression: sync endpoints must accept API tokens (not just sync tokens).
/// Seaf-daemon 9.0.4 uses the API token from /api2/auth-token/ for initial
/// /seafhttp/ requests during the check-server phase.
#[tokio::test]
async fn test_sync_endpoint_accepts_api_token() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let api_token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, api_token, "API Auth Test").await;

    // All sync endpoints should accept the API token (not just sync tokens)
    let resp = client.get_head_commit(api_token, &repo_id).await;
    assert_eq!(resp.status(), 200);

    let resp = client
        .permission_check(api_token, &repo_id, "download")
        .await;
    assert_eq!(resp.status(), 200);

    let resp = client
        .fs_id_list(
            api_token,
            &repo_id,
            "0000000000000000000000000000000000000000",
        )
        .await;
    assert_eq!(resp.status(), 200);
}

/// Regression: empty repos must return zero-commit as HEAD, not corrupted flag.
/// Seaf-daemon checks "Response doesn't contain head commit id" and fails.
#[tokio::test]
async fn test_zero_commit_for_empty_repo() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, token, "Empty Repo").await;
    let sync_token = get_sync_token(&client, token, &repo_id).await;

    let resp = client.get_head_commit(&sync_token, &repo_id).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["is_corrupted"].as_i64().unwrap(), 0);
    assert_eq!(
        body["head_commit_id"].as_str().unwrap(),
        "0000000000000000000000000000000000000000"
    );

    // Fetching the zero commit should return valid JSON (not 404)
    let resp = client
        .get_commit(
            &sync_token,
            &repo_id,
            "0000000000000000000000000000000000000000",
        )
        .await;
    assert_eq!(resp.status(), 200);
    let commit_body: serde_json::Value =
        serde_json::from_slice(&resp.bytes().await.unwrap()).unwrap();
    assert_eq!(
        commit_body["commit_id"].as_str().unwrap(),
        "0000000000000000000000000000000000000000"
    );
}

/// Regression: pack-fs must accept JSON array bodies (seaf-daemon 9.0.4 format).
#[tokio::test]
async fn test_pack_fs_accepts_json_array() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, token, "JSON Body Test").await;
    let sync_token = get_sync_token(&client, token, &repo_id).await;

    // First push some fs objects via recv-fs
    let empty_dir = base::common::FsDirData {
        dirents: vec![],
        obj_type: 3,
        version: 1,
    };
    let dir_json = serde_json::to_string(&empty_dir).unwrap();
    let dir_fs_id = infra::crypto::fs_id::sha1_hex(dir_json.as_bytes());
    let dir_compressed =
        infra::serialization::pack_fs::compress_fs_data(dir_json.as_bytes()).unwrap();

    let mut packed = Vec::new();
    packed.extend_from_slice(dir_fs_id.as_bytes());
    packed.extend_from_slice(&(dir_compressed.len() as u32).to_be_bytes());
    packed.extend_from_slice(&dir_compressed);

    let resp = client.recv_fs(&sync_token, &repo_id, packed).await;
    assert_eq!(resp.status(), 200);

    // Call pack-fs with JSON array body (the seaf-daemon 9.0.4 format)
    let resp = client.pack_fs(&sync_token, &repo_id, &[&dir_fs_id]).await;
    assert_eq!(resp.status(), 200);

    let data = resp.bytes().await.unwrap();
    assert!(!data.is_empty());

    // Decode and verify the entry
    let decoded = infra::serialization::pack_fs::decode_pack_fs_entries(&data).unwrap();
    assert_eq!(decoded.len(), 1);
    assert_eq!(decoded[0].0, dir_fs_id);
}

/// Regression: pack-fs must also accept form-encoded bodies (older seafile clients).
#[tokio::test]
async fn test_pack_fs_accepts_form_encoded() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, token, "Form Body Test").await;
    let sync_token = get_sync_token(&client, token, &repo_id).await;

    // Push fs object
    let empty_dir = base::common::FsDirData {
        dirents: vec![],
        obj_type: 3,
        version: 1,
    };
    let dir_json = serde_json::to_string(&empty_dir).unwrap();
    let dir_fs_id = infra::crypto::fs_id::sha1_hex(dir_json.as_bytes());
    let dir_compressed =
        infra::serialization::pack_fs::compress_fs_data(dir_json.as_bytes()).unwrap();

    let mut packed = Vec::new();
    packed.extend_from_slice(dir_fs_id.as_bytes());
    packed.extend_from_slice(&(dir_compressed.len() as u32).to_be_bytes());
    packed.extend_from_slice(&dir_compressed);
    client.recv_fs(&sync_token, &repo_id, packed).await;

    // Send form-encoded request (older seafile client format)
    let form_body = format!("fs_ids={}", dir_fs_id);
    let resp = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!(
            "{}/seafhttp/repo/{}/pack-fs/",
            server.base_url, repo_id
        ))
        .header("Seafile-Repo-Token", sync_token)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form_body)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let data = resp.bytes().await.unwrap();
    assert!(!data.is_empty());
}

/// Regression: check-blocks must accept JSON array bodies.
#[tokio::test]
async fn test_check_blocks_accepts_json_array() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, token, "Blocks JSON Test").await;
    let sync_token = get_sync_token(&client, token, &repo_id).await;

    // Upload a real block first
    let block_data = b"json test block";
    use sha1::{Digest, Sha1};
    let mut hasher = Sha1::new();
    hasher.update(block_data);
    let real_id = hex::encode(hasher.finalize());
    client
        .put_block(&sync_token, &repo_id, &real_id, block_data.to_vec())
        .await;

    // Call check-blocks with JSON array body
    let resp = client
        .check_blocks(
            &sync_token,
            &repo_id,
            &[&real_id, "cccccccccccccccccccccccccccccccccccccccc"],
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let missing = body.as_array().unwrap();
    // Only the nonexistent (but well-formed) block should be reported missing
    assert_eq!(missing.len(), 1);
}

/// Regression: check-blocks must accept form-encoded bodies (older clients).
#[tokio::test]
async fn test_check_blocks_accepts_form_encoded() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, token, "Blocks Form Test").await;
    let sync_token = get_sync_token(&client, token, &repo_id).await;

    // Upload a block
    let block_data = b"form test block";
    use sha1::{Digest, Sha1};
    let mut hasher = Sha1::new();
    hasher.update(block_data);
    let real_id = hex::encode(hasher.finalize());
    client
        .put_block(&sync_token, &repo_id, &real_id, block_data.to_vec())
        .await;

    // Send form-encoded request
    let form_body = format!(
        "block_ids={}&block_ids=dddddddddddddddddddddddddddddddddddddddd",
        real_id
    );
    let resp = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!(
            "{}/seafhttp/repo/{}/check-blocks/",
            server.base_url, repo_id
        ))
        .header("Seafile-Repo-Token", sync_token)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form_body)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let missing = body.as_array().unwrap();
    assert_eq!(missing.len(), 1);
}

/// Regression: check-fs must accept form-encoded bodies.
#[tokio::test]
async fn test_check_fs_accepts_form_encoded() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, token, "FS Form Test").await;
    let sync_token = get_sync_token(&client, token, &repo_id).await;

    // Push an fs object
    let empty_dir = base::common::FsDirData {
        dirents: vec![],
        obj_type: 3,
        version: 1,
    };
    let dir_json = serde_json::to_string(&empty_dir).unwrap();
    let dir_fs_id = infra::crypto::fs_id::sha1_hex(dir_json.as_bytes());
    let dir_compressed =
        infra::serialization::pack_fs::compress_fs_data(dir_json.as_bytes()).unwrap();

    let mut packed = Vec::new();
    packed.extend_from_slice(dir_fs_id.as_bytes());
    packed.extend_from_slice(&(dir_compressed.len() as u32).to_be_bytes());
    packed.extend_from_slice(&dir_compressed);
    client.recv_fs(&sync_token, &repo_id, packed).await;

    // Send form-encoded check-fs
    let form_body = format!(
        "fs_ids={}&fs_ids=fake_missing_40char_hex_id_0000bb",
        dir_fs_id
    );
    let resp = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!(
            "{}/seafhttp/repo/{}/check-fs/",
            server.base_url, repo_id
        ))
        .header("Seafile-Repo-Token", sync_token)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form_body)
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let missing = body.as_array().unwrap();
    // Only the fake one should be missing
    assert_eq!(missing.len(), 1);
    assert!(missing[0].as_str().unwrap().starts_with("fake_missing"));
}

/// Regression: Commit JSON from get_commit must include all fields the
/// seaf-daemon expects. Missing root_id or creator causes protocol errors.
#[tokio::test]
async fn test_regression_commit_has_required_fields() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "commit@test.com", "password123").await;
    let resp = client.login("commit@test.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let api_token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, api_token, "Commit Test").await;
    let sync_token = get_sync_token(&client, api_token, &repo_id).await;

    // Upload a file to create a real commit
    client
        .upload_file(api_token, &repo_id, "/", "commit_file.txt", b"test")
        .await;

    let resp = client.get_head_commit(&sync_token, &repo_id).await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let head = body["head_commit_id"].as_str().unwrap();

    let resp = client.get_commit(&sync_token, &repo_id, head).await;
    assert_eq!(resp.status(), 200);
    let data = resp.bytes().await.unwrap();

    // Parse commit JSON
    let commit: serde_json::Value = serde_json::from_slice(&data).unwrap();

    // Required fields for seaf-daemon parsing
    assert!(
        commit["commit_id"].is_string(),
        "commit must have commit_id"
    );
    assert!(commit["root_id"].is_string(), "commit must have root_id");
    assert!(commit["repo_id"].is_string(), "commit must have repo_id");
    assert!(
        commit["creator_name"].is_string(),
        "commit must have creator_name"
    );
    assert!(commit["creator"].is_string(), "commit must have creator");
    assert!(commit["ctime"].is_number(), "commit must have ctime");

    // creator must be 40-char hex (seaf-daemon sanity check: strlen(creator) == 40)
    let creator = commit["creator"].as_str().unwrap();
    assert_eq!(creator.len(), 40, "creator must be exactly 40 hex chars");
    assert!(
        creator.chars().all(|c| c.is_ascii_hexdigit()),
        "creator must be hex"
    );

    // root_id must be 40 hex chars
    let root_id = commit["root_id"].as_str().unwrap();
    assert_eq!(root_id.len(), 40, "root_id must be exactly 40 hex chars");
}

/// Security: check-blocks must reject malformed / path-traversal block ids
/// with a 400 instead of probing arbitrary file paths on the server.
#[tokio::test]
async fn test_check_blocks_rejects_malicious_block_ids() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .check_blocks(&f.sync_token, &f.repo_id, &["../../../../etc/passwd"])
        .await;
    assert_eq!(resp.status(), 400);
}

// ==================== Security: cross-repo IDOR ====================

/// Security: a sync token bound to repo B must not be usable on repo A.
#[tokio::test]
async fn test_sync_token_rejected_for_other_repo() {
    let f = TestFixture::new().await;

    create_test_user(f.server.db.as_ref(), "other@example.com", "password123").await;
    let resp = f.client.login("other@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let b_api = body["token"].as_str().unwrap().to_string();
    let b_repo = common::create_test_repo(&f.client, &b_api, "B Repo").await;
    let b_sync = get_sync_token(&f.client, &b_api, &b_repo).await;

    // B's sync token (bound to b_repo) must be rejected on A's repo.
    let resp = f.client.get_head_commit(&b_sync, &f.repo_id).await;
    assert!(
        resp.status() == 401 || resp.status() == 403,
        "expected 401/403 for cross-repo get_head_commit, got {}",
        resp.status()
    );

    // …and write paths too.
    let resp = f
        .client
        .get_commit(
            &b_sync,
            &f.repo_id,
            "0000000000000000000000000000000000000000",
        )
        .await;
    assert!(
        resp.status() == 401 || resp.status() == 403,
        "expected 401/403 for cross-repo get_commit, got {}",
        resp.status()
    );
}

/// Security: an API token must not grant access to a repo the user is not a
/// member of.
#[tokio::test]
async fn test_api_token_rejected_for_other_repo() {
    let f = TestFixture::new().await;

    create_test_user(f.server.db.as_ref(), "other@example.com", "password123").await;
    let resp = f.client.login("other@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let b_api = body["token"].as_str().unwrap().to_string();

    let resp = f.client.get_head_commit(&b_api, &f.repo_id).await;
    assert!(
        resp.status() == 401 || resp.status() == 403,
        "expected 401/403 for cross-repo API token, got {}",
        resp.status()
    );
}

/// Security: repo-tokens must not mint a token for a repo the caller is not a
/// member of (official seahub skips such repos rather than erroring).
#[tokio::test]
async fn test_repo_tokens_skips_non_member() {
    let f = TestFixture::new().await;

    create_test_user(f.server.db.as_ref(), "other@example.com", "password123").await;
    let resp = f.client.login("other@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let b_api = body["token"].as_str().unwrap().to_string();

    let path = format!("/api2/repo-tokens/?repos={}", f.repo_id);
    let resp = f.client.get(&path, Some(&b_api)).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body.get(&f.repo_id).is_none(),
        "non-member must not receive a sync token for someone else's repo"
    );
}

/// Regression: recv_fs must accept directory objects with type=3 (not 2).
/// The seaf-daemon sends type=3 for directories; rejecting it would break
/// the upload sync flow.
#[tokio::test]
async fn test_regression_recv_fs_accepts_dir_type_3() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "dirtype@test.com", "password123").await;
    let resp = client.login("dirtype@test.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let api_token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, api_token, "DirType Test").await;
    let sync_token = get_sync_token(&client, api_token, &repo_id).await;

    use base::common::{FsDirData, FsFileData};
    use infra::serialization::pack_fs;

    // Create a directory with type=3 (what seaf-daemon actually sends)
    let file_data = FsFileData {
        block_ids: vec!["abc123abc123abc123abc123abc123abc123abc1".to_string()],
        size: 100,
        obj_type: 1, // file
        version: 1,
    };
    let file_fs_id =
        infra::crypto::fs_id::sha1_hex(serde_json::to_string(&file_data).unwrap().as_bytes());
    let file_json = serde_json::to_string(&file_data).unwrap();
    let file_compressed = pack_fs::compress_fs_data(file_json.as_bytes()).unwrap();

    let dir_data = FsDirData {
        dirents: vec![base::common::DirEntryData {
            id: file_fs_id.clone(),
            mode: 33188,
            modifier: "test@example.com".to_string(),
            mtime: 1700000000,
            name: "test.txt".to_string(),
            size: 100,
        }],
        obj_type: 3, // dir — this must be 3, not 2
        version: 1,
    };
    let dir_fs_id =
        infra::crypto::fs_id::sha1_hex(serde_json::to_string(&dir_data).unwrap().as_bytes());
    let dir_json = serde_json::to_string(&dir_data).unwrap();
    let dir_compressed = pack_fs::compress_fs_data(dir_json.as_bytes()).unwrap();

    // Upload both via recv_fs
    let mut packed = Vec::new();
    packed.extend_from_slice(dir_fs_id.as_bytes());
    packed.extend_from_slice(&(dir_compressed.len() as u32).to_be_bytes());
    packed.extend_from_slice(&dir_compressed);
    packed.extend_from_slice(file_fs_id.as_bytes());
    packed.extend_from_slice(&(file_compressed.len() as u32).to_be_bytes());
    packed.extend_from_slice(&file_compressed);

    let resp = client.recv_fs(&sync_token, &repo_id, packed).await;
    assert_eq!(resp.status(), 200);

    // Verify they are stored and retrievable
    let resp = client
        .check_fs(&sync_token, &repo_id, &[&dir_fs_id, &file_fs_id])
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let missing = body.as_array().unwrap();
    assert!(
        missing.is_empty(),
        "both dir (type=3) and file (type=1) must be accepted"
    );

    // Verify pack_fs returns them with correct types
    let resp = client
        .pack_fs(&sync_token, &repo_id, &[&dir_fs_id, &file_fs_id])
        .await;
    assert_eq!(resp.status(), 200);
    let packed = resp.bytes().await.unwrap();
    let entries = pack_fs::decode_pack_fs_entries(&packed).unwrap();
    assert_eq!(entries.len(), 2);

    for (fs_id, data) in &entries {
        let decompressed = pack_fs::decompress_fs_data(data).unwrap();
        let json_val: serde_json::Value = serde_json::from_slice(&decompressed).unwrap();
        let obj_type = json_val["type"].as_i64().unwrap();
        if fs_id == &dir_fs_id {
            assert_eq!(obj_type, 3, "dir must have type=3");
        } else {
            assert_eq!(obj_type, 1, "file must have type=1");
        }
    }
}
