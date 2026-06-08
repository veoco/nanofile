mod common;

use common::{TestServer, create_test_user, get_sync_token};
use nanofile::serialization::commit_json::CommitData;
use nanofile::serialization::fs_json::{DirEntryData, FsDirData, FsFileData};
use nanofile::serialization::pack_fs;

fn random_hex_id() -> String {
    use sha1::{Digest, Sha1};
    let mut hasher = Sha1::new();
    hasher.update(rand::random::<[u8; 32]>());
    hex::encode(hasher.finalize())
}

fn make_dir_fs_data(entries: Vec<DirEntryData>) -> FsDirData {
    FsDirData {
        dirents: entries,
        obj_type: 3,
        version: 1,
    }
}

fn make_file_fs_data(block_ids: Vec<String>, size: i64) -> FsFileData {
    FsFileData {
        block_ids,
        size,
        obj_type: 1,
        version: 1,
    }
}

async fn setup_repo() -> (TestServer, String, String, String) {
    let server = TestServer::start().await;
    let client = server.client();
    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let api_token = body["token"].as_str().unwrap().to_string();
    let repo_id = common::create_test_repo(&client, &api_token, "FS Test").await;
    let sync_token = get_sync_token(&client, &api_token, &repo_id).await;
    (server, api_token, repo_id, sync_token)
}

async fn push_commit(
    client: &common::client::TestClient,
    sync_token: &str,
    repo_id: &str,
    root_id: &str,
    parent_id: Option<&str>,
) -> String {
    let commit_id = random_hex_id();
    let now = chrono::Utc::now().timestamp();
    let commit_data = CommitData {
        commit_id: commit_id.clone(),
        repo_id: repo_id.to_string(),
        root_id: root_id.to_string(),
        creator_name: "test@example.com".to_string(),
        creator: "0000000000000000000000000000000000000000".to_string(),
        description: "test commit".to_string(),
        ctime: now,
        parent_id: parent_id.map(|s| s.to_string()),
        second_parent_id: None,
        repo_name: None,
        repo_desc: None,
        repo_category: None,
        encrypted: None,
        enc_version: None,
        magic: None,
        key: None,
        version: 1,
    };
    let json_str = commit_data.to_compact_json();
    let resp = client
        .put_commit(sync_token, repo_id, &commit_id, json_str.into_bytes())
        .await;
    assert_eq!(resp.status(), 200);
    let resp = client.update_branch(sync_token, repo_id, &commit_id).await;
    assert_eq!(resp.status(), 200);
    commit_id
}

#[tokio::test]
async fn test_fs_dir_type_value() {
    let dir_data = make_dir_fs_data(vec![]);
    let json = dir_data.to_compact_json();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["type"].as_i64().unwrap(), 3);
}

#[tokio::test]
async fn test_fs_file_type_value() {
    let file_data = make_file_fs_data(vec!["abc123".to_string()], 1024);
    let json = file_data.to_compact_json();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["type"].as_i64().unwrap(), 1);
}

#[tokio::test]
async fn test_fs_dir_serialization_roundtrip() {
    let dir_data = make_dir_fs_data(vec![DirEntryData {
        id: random_hex_id(),
        mode: 33188,
        modifier: "user@example.com".to_string(),
        mtime: 1700000000,
        name: "test.txt".to_string(),
        size: 1024,
    }]);

    let fs_id = dir_data.compute_fs_id();
    assert_eq!(fs_id.len(), 40);

    let json = dir_data.to_compact_json();
    let compressed = pack_fs::compress_fs_data(json.as_bytes()).unwrap();
    let decompressed = pack_fs::decompress_fs_data(&compressed).unwrap();
    let roundtripped: FsDirData = serde_json::from_slice(&decompressed).unwrap();

    assert_eq!(roundtripped.obj_type, 3);
    assert_eq!(roundtripped.version, 1);
    assert_eq!(roundtripped.dirents.len(), 1);
    assert_eq!(roundtripped.dirents[0].name, "test.txt");
    assert_eq!(roundtripped.dirents[0].size, 1024);
}

#[tokio::test]
async fn test_fs_file_serialization_roundtrip() {
    let block_id = random_hex_id();
    let file_data = make_file_fs_data(vec![block_id.clone()], 4096);

    let fs_id = file_data.compute_fs_id();
    assert_eq!(fs_id.len(), 40);

    let json = file_data.to_compact_json();
    let compressed = pack_fs::compress_fs_data(json.as_bytes()).unwrap();
    let decompressed = pack_fs::decompress_fs_data(&compressed).unwrap();
    let roundtripped: FsFileData = serde_json::from_slice(&decompressed).unwrap();

    assert_eq!(roundtripped.obj_type, 1);
    assert_eq!(roundtripped.version, 1);
    assert_eq!(roundtripped.size, 4096);
    assert_eq!(roundtripped.block_ids.len(), 1);
    assert_eq!(roundtripped.block_ids[0], block_id);
}

#[tokio::test]
async fn test_recv_fs_binary_format() {
    let (server, _api_token, repo_id, sync_token) = setup_repo().await;
    let client = server.client();

    let file_data = make_file_fs_data(vec![random_hex_id()], 512);
    let fs_id = file_data.compute_fs_id();
    let json = file_data.to_compact_json();
    let compressed = pack_fs::compress_fs_data(json.as_bytes()).unwrap();

    let mut packed = Vec::new();
    packed.extend_from_slice(fs_id.as_bytes());
    packed.extend_from_slice(&(compressed.len() as u32).to_be_bytes());
    packed.extend_from_slice(&compressed);

    let resp = client.recv_fs(&sync_token, &repo_id, packed).await;
    assert_eq!(resp.status(), 200);

    let root_dir = make_dir_fs_data(vec![DirEntryData {
        id: fs_id.clone(),
        mode: 33188,
        modifier: "test@example.com".to_string(),
        mtime: chrono::Utc::now().timestamp(),
        name: "test.txt".to_string(),
        size: 512,
    }]);
    let root_json = root_dir.to_compact_json();
    let root_compressed = pack_fs::compress_fs_data(root_json.as_bytes()).unwrap();
    let root_fs_id = root_dir.compute_fs_id();

    let mut root_packed = Vec::new();
    root_packed.extend_from_slice(root_fs_id.as_bytes());
    root_packed.extend_from_slice(&(root_compressed.len() as u32).to_be_bytes());
    root_packed.extend_from_slice(&root_compressed);

    let resp = client.recv_fs(&sync_token, &repo_id, root_packed).await;
    assert_eq!(resp.status(), 200);

    push_commit(&client, &sync_token, &repo_id, &root_fs_id, None).await;
}

#[tokio::test]
async fn test_recv_fs_invalid_obj_id() {
    let (server, _api_token, repo_id, sync_token) = setup_repo().await;
    let client = server.client();

    let mut packed = Vec::new();
    packed.extend_from_slice(b"not-a-valid-hex-id!"); // 20 bytes, invalid
    packed.resize(40, b'0');
    packed.extend_from_slice(&1u32.to_be_bytes());
    packed.push(0x00);

    let resp = client.recv_fs(&sync_token, &repo_id, packed).await;
    assert_eq!(resp.status(), 500);
}

#[tokio::test]
async fn test_recv_fs_truncated_body() {
    let (server, _api_token, repo_id, sync_token) = setup_repo().await;
    let client = server.client();

    let packed = vec![0u8; 20];
    let resp = client.recv_fs(&sync_token, &repo_id, packed).await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_pack_fs_returns_packed_binary() {
    let (server, _api_token, repo_id, sync_token) = setup_repo().await;
    let client = server.client();

    let file_data = make_file_fs_data(vec![random_hex_id()], 256);
    let fs_id = file_data.compute_fs_id();
    let json = file_data.to_compact_json();
    let compressed = pack_fs::compress_fs_data(json.as_bytes()).unwrap();

    let mut packed = Vec::new();
    packed.extend_from_slice(fs_id.as_bytes());
    packed.extend_from_slice(&(compressed.len() as u32).to_be_bytes());
    packed.extend_from_slice(&compressed);
    let resp = client.recv_fs(&sync_token, &repo_id, packed).await;
    assert_eq!(resp.status(), 200);

    let resp = client.pack_fs(&sync_token, &repo_id, &[&fs_id]).await;
    assert_eq!(resp.status(), 200);
    let body = resp.bytes().await.unwrap();
    assert!(!body.is_empty());

    let entries = pack_fs::decode_pack_fs_entries(&body).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].0, fs_id);
    assert_eq!(entries[0].1, compressed);
}

#[tokio::test]
async fn test_check_fs_partial_exists() {
    let (server, _api_token, repo_id, sync_token) = setup_repo().await;
    let client = server.client();

    let file_data = make_file_fs_data(vec![random_hex_id()], 100);
    let fs_id = file_data.compute_fs_id();
    let json = file_data.to_compact_json();
    let compressed = pack_fs::compress_fs_data(json.as_bytes()).unwrap();

    let mut packed = Vec::new();
    packed.extend_from_slice(fs_id.as_bytes());
    packed.extend_from_slice(&(compressed.len() as u32).to_be_bytes());
    packed.extend_from_slice(&compressed);
    let resp = client.recv_fs(&sync_token, &repo_id, packed).await;
    assert_eq!(resp.status(), 200);

    let fake_id = random_hex_id();
    let resp = client
        .check_fs(&sync_token, &repo_id, &[&fs_id, &fake_id])
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let missing = body.as_array().unwrap();
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].as_str().unwrap(), fake_id);
}

#[tokio::test]
async fn test_fs_id_list_with_server_head() {
    let (server, _api_token, repo_id, sync_token) = setup_repo().await;
    let client = server.client();

    let root_dir = make_dir_fs_data(vec![DirEntryData {
        id: random_hex_id(),
        mode: 33188,
        modifier: "test@example.com".to_string(),
        mtime: chrono::Utc::now().timestamp(),
        name: "file.txt".to_string(),
        size: 100,
    }]);
    let root_fs_id = root_dir.compute_fs_id();
    let root_json = root_dir.to_compact_json();
    let root_compressed = pack_fs::compress_fs_data(root_json.as_bytes()).unwrap();

    let mut packed = Vec::new();
    packed.extend_from_slice(root_fs_id.as_bytes());
    packed.extend_from_slice(&(root_compressed.len() as u32).to_be_bytes());
    packed.extend_from_slice(&root_compressed);
    let resp = client.recv_fs(&sync_token, &repo_id, packed).await;
    assert_eq!(resp.status(), 200);

    let commit_id = push_commit(&client, &sync_token, &repo_id, &root_fs_id, None).await;

    let resp = client.fs_id_list(&sync_token, &repo_id, &commit_id).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let fs_ids = body.as_array().unwrap();
    assert!(fs_ids.contains(&serde_json::Value::String(root_fs_id)));
}

#[tokio::test]
async fn test_fs_id_list_with_matching_client_head() {
    let (server, _api_token, repo_id, sync_token) = setup_repo().await;
    let client = server.client();

    let root_dir = make_dir_fs_data(vec![]);
    let root_fs_id = root_dir.compute_fs_id();
    let root_json = root_dir.to_compact_json();
    let root_compressed = pack_fs::compress_fs_data(root_json.as_bytes()).unwrap();

    let mut packed = Vec::new();
    packed.extend_from_slice(root_fs_id.as_bytes());
    packed.extend_from_slice(&(root_compressed.len() as u32).to_be_bytes());
    packed.extend_from_slice(&root_compressed);
    let resp = client.recv_fs(&sync_token, &repo_id, packed).await;
    assert_eq!(resp.status(), 200);

    let commit_id = push_commit(&client, &sync_token, &repo_id, &root_fs_id, None).await;

    let resp = client
        .fs_id_list_with_client(&sync_token, &repo_id, &commit_id, &commit_id)
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let fs_ids = body.as_array().unwrap();
    assert!(fs_ids.is_empty());
}

#[tokio::test]
async fn test_check_blocks_partial_exists() {
    let (server, _api_token, repo_id, sync_token) = setup_repo().await;
    let client = server.client();

    let block_data = b"test block data for checking";
    use sha1::{Digest, Sha1};
    let mut hasher = Sha1::new();
    hasher.update(block_data);
    let block_id = hex::encode(hasher.finalize());
    let resp = client
        .put_block(&sync_token, &repo_id, &block_id, block_data.to_vec())
        .await;
    assert_eq!(resp.status(), 200);

    let fake_block = random_hex_id();
    let resp = client
        .check_blocks(&sync_token, &repo_id, &[&block_id, &fake_block])
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let missing = body.as_array().unwrap();
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].as_str().unwrap(), fake_block);
}

/// Regression: SEAF_METADATA_TYPE_DIR must be 3, not 2.
/// The seaf-daemon checks `type == 3` for directory objects; using 2 caused
/// "Failed to find dir" errors and silent sync failures.
#[tokio::test]
async fn test_regression_dir_metadata_type_is_3() {
    use nanofile::serialization::fs_json::{SEAF_METADATA_TYPE_DIR, SEAF_METADATA_TYPE_FILE};
    assert_eq!(
        SEAF_METADATA_TYPE_DIR, 3,
        "SEAF_METADATA_TYPE_DIR must be 3"
    );
    assert_eq!(
        SEAF_METADATA_TYPE_FILE, 1,
        "SEAF_METADATA_TYPE_FILE must be 1"
    );
    // Ensure dir and file types are not confused
    assert_ne!(SEAF_METADATA_TYPE_DIR, SEAF_METADATA_TYPE_FILE);
}

/// Regression: When seaf-daemon uploads via recv_fs with type=3 (dir), the
/// stored obj_type must be preserved correctly in the database.
#[tokio::test]
async fn test_regression_recv_fs_stores_correct_dir_type() {
    let (server, _api_token, repo_id, sync_token) = setup_repo().await;
    let client = server.client();

    // Create a dir fs object with type=3 (what seaf-daemon sends)
    let dir_data = make_dir_fs_data(vec![]);
    let dir_json = dir_data.to_compact_json();
    let dir_compressed = pack_fs::compress_fs_data(dir_json.as_bytes()).unwrap();
    let dir_fs_id = dir_data.compute_fs_id();

    let mut packed = Vec::new();
    packed.extend_from_slice(dir_fs_id.as_bytes());
    packed.extend_from_slice(&(dir_compressed.len() as u32).to_be_bytes());
    packed.extend_from_slice(&dir_compressed);

    client.recv_fs(&sync_token, &repo_id, packed).await;

    // Push a commit with this root
    push_commit(&client, &sync_token, &repo_id, &dir_fs_id, None).await;

    // Now fetch via pack_fs and verify the decompressed JSON has type=3
    let resp = client.pack_fs(&sync_token, &repo_id, &[&dir_fs_id]).await;
    assert_eq!(resp.status(), 200);
    let packed = resp.bytes().await.unwrap();
    let entries = pack_fs::decode_pack_fs_entries(&packed).unwrap();
    assert_eq!(entries.len(), 1);

    let decompressed = pack_fs::decompress_fs_data(&entries[0].1).unwrap();
    let json_val: serde_json::Value = serde_json::from_slice(&decompressed).unwrap();
    assert_eq!(
        json_val["type"].as_i64().unwrap(),
        3,
        "recv_fs must preserve dir obj_type=3"
    );
}

/// Regression: file fs objects must have type=1 in their JSON, matching
/// SEAF_METADATA_TYPE_FILE.
#[tokio::test]
async fn test_regression_recv_fs_stores_correct_file_type() {
    let (server, _api_token, repo_id, sync_token) = setup_repo().await;
    let client = server.client();

    let file_data = make_file_fs_data(vec![random_hex_id()], 256);
    let file_json = file_data.to_compact_json();
    let file_compressed = pack_fs::compress_fs_data(file_json.as_bytes()).unwrap();
    let file_fs_id = file_data.compute_fs_id();

    let mut packed = Vec::new();
    packed.extend_from_slice(file_fs_id.as_bytes());
    packed.extend_from_slice(&(file_compressed.len() as u32).to_be_bytes());
    packed.extend_from_slice(&file_compressed);

    client.recv_fs(&sync_token, &repo_id, packed).await;

    let resp = client.pack_fs(&sync_token, &repo_id, &[&file_fs_id]).await;
    assert_eq!(resp.status(), 200);
    let packed = resp.bytes().await.unwrap();
    let entries = pack_fs::decode_pack_fs_entries(&packed).unwrap();
    assert_eq!(entries.len(), 1);

    let decompressed = pack_fs::decompress_fs_data(&entries[0].1).unwrap();
    let json_val: serde_json::Value = serde_json::from_slice(&decompressed).unwrap();
    assert_eq!(
        json_val["type"].as_i64().unwrap(),
        1,
        "recv_fs must preserve file obj_type=1"
    );
}

/// Regression: API file upload via multipart must produce fs objects with
/// type=3 for the parent directory and type=1 for the file.
#[tokio::test]
async fn test_regression_api_upload_produces_correct_obj_types() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "objtype@test.com", "password123").await;
    let resp = client.login("objtype@test.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let api_token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, api_token, "ObjType Test").await;
    let sync_token = get_sync_token(&client, api_token, &repo_id).await;

    // Upload a file via the API
    let resp = client
        .upload_file(
            api_token,
            &repo_id,
            "/",
            "objtype_check.txt",
            b"test content",
        )
        .await;
    assert_eq!(resp.status(), 200);

    // Get the head commit
    let resp = client.get_head_commit(&sync_token, &repo_id).await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let head_commit_id = body["head_commit_id"].as_str().unwrap().to_string();

    // Get fs_id_list for the commit
    let resp = client
        .fs_id_list(&sync_token, &repo_id, &head_commit_id)
        .await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let fs_ids: Vec<String> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();

    // Fetch all fs objects and verify their types
    let fs_refs: Vec<&str> = fs_ids.iter().map(|s| s.as_str()).collect();
    let resp = client.pack_fs(&sync_token, &repo_id, &fs_refs).await;
    assert_eq!(resp.status(), 200);
    let packed = resp.bytes().await.unwrap();
    let entries = pack_fs::decode_pack_fs_entries(&packed).unwrap();

    let mut found_dir = false;
    let mut found_file = false;
    for (_fs_id, data) in &entries {
        let decompressed = pack_fs::decompress_fs_data(data).unwrap();
        let json_val: serde_json::Value = serde_json::from_slice(&decompressed).unwrap();
        let obj_type = json_val["type"].as_i64().unwrap();
        if obj_type == 3 {
            found_dir = true;
            // Must have dirents array
            assert!(
                json_val["dirents"].is_array(),
                "dir object must have dirents"
            );
        } else if obj_type == 1 {
            found_file = true;
            // Must have block_ids array
            assert!(
                json_val["block_ids"].is_array(),
                "file object must have block_ids"
            );
        }
    }
    assert!(
        found_dir,
        "API upload must produce a directory fs object with type=3"
    );
    assert!(
        found_file,
        "API upload must produce a file fs object with type=1"
    );
}

/// Regression: The fs_id (SHA1 hash) must be computed from the sorted-key JSON,
/// including the correct type field. If type changes, the fs_id changes.
/// This test ensures the hash is stable.
#[tokio::test]
async fn test_regression_fs_id_is_stable() {
    // Create a known dir and compute its fs_id
    let dir_data = FsDirData {
        dirents: vec![],
        obj_type: 3,
        version: 1,
    };

    let fs_id_1 = dir_data.compute_fs_id();
    let fs_id_2 = dir_data.compute_fs_id();

    // Same data must produce same fs_id
    assert_eq!(fs_id_1, fs_id_2, "fs_id must be deterministic");
    assert_eq!(fs_id_1.len(), 40, "fs_id must be 40 hex chars");

    // A different type value must produce a different fs_id
    let dir_data_wrong_type = FsDirData {
        dirents: vec![],
        obj_type: 2, // wrong type (SEAF_METADATA_TYPE_LINK)
        version: 1,
    };
    let fs_id_wrong = dir_data_wrong_type.compute_fs_id();
    assert_ne!(
        fs_id_1, fs_id_wrong,
        "Different type values must produce different fs_ids"
    );
}

/// Regression: Protocol version must be 2, matching the real seafile server.
#[tokio::test]
async fn test_regression_protocol_version_is_2() {
    let server = TestServer::start().await;
    let client = server.client();
    let resp = client.protocol_version().await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["version"].as_i64().unwrap(), 2);
}

/// Regression: Zero-commit handling for empty repos must return is_corrupted=0
/// and a valid zero head_commit_id.
#[tokio::test]
async fn test_regression_empty_repo_has_valid_head() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "empty@test.com", "password123").await;
    let resp = client.login("empty@test.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let api_token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, api_token, "Empty Head Test").await;
    let sync_token = get_sync_token(&client, api_token, &repo_id).await;

    let resp = client.get_head_commit(&sync_token, &repo_id).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["is_corrupted"].as_i64().unwrap(), 0);
    assert_eq!(
        body["head_commit_id"].as_str().unwrap(),
        "0000000000000000000000000000000000000000"
    );
}

/// Regression: Download-info must include all fields the seaf-cli requires.
/// Missing fields cause KeyError in the Python client.
#[tokio::test]
async fn test_regression_download_info_has_required_fields() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "dlinfo@test.com", "password123").await;
    let resp = client.login("dlinfo@test.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let api_token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, api_token, "DL Info Test").await;

    let resp = client.download_info(api_token, &repo_id).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();

    // Required fields (KeyError in seaf-cli if missing)
    assert!(
        body["repo_version"].as_i64().is_some(),
        "repo_version is required"
    );
    assert_eq!(
        body["repo_version"].as_i64().unwrap(),
        1,
        "repo_version must be 1"
    );
    assert!(
        !body["email"].as_str().unwrap_or("").is_empty(),
        "email must not be empty"
    );
    assert!(
        !body["repo_name"].as_str().unwrap_or("").is_empty(),
        "repo_name must not be empty"
    );
    assert!(
        body["token"].as_str().unwrap_or("").len() >= 40,
        "sync token must be at least 40 chars"
    );

    // Must be present (even if null)
    assert!(body["encrypted"].is_string(), "encrypted must be a string");
    assert!(
        body["permission"].is_string(),
        "permission must be a string"
    );
    // salt may be null or string
    assert!(
        body["salt"].is_null() || body["salt"].is_string(),
        "salt must be null or string"
    );
}

/// Regression: pack_fs must return data in the exact binary format that
/// seaf-daemon expects: [40-byte fs_id][4-byte big-endian size][zlib data].
#[tokio::test]
async fn test_regression_pack_fs_binary_format() {
    let (server, _api_token, repo_id, sync_token) = setup_repo().await;
    let client = server.client();

    // Upload a file fs object
    let file_data = make_file_fs_data(vec![random_hex_id()], 128);
    let fs_id = file_data.compute_fs_id();
    let json = file_data.to_compact_json();
    let compressed = pack_fs::compress_fs_data(json.as_bytes()).unwrap();

    let mut packed = Vec::new();
    packed.extend_from_slice(fs_id.as_bytes());
    packed.extend_from_slice(&(compressed.len() as u32).to_be_bytes());
    packed.extend_from_slice(&compressed);
    client.recv_fs(&sync_token, &repo_id, packed).await;

    // Fetch via pack_fs
    let resp = client.pack_fs(&sync_token, &repo_id, &[&fs_id]).await;
    assert_eq!(resp.status(), 200);
    let body = resp.bytes().await.unwrap();

    // Must be at least 44 bytes (40 id + 4 size)
    assert!(body.len() >= 44, "pack_fs response too short");

    // Parse header
    let returned_id = String::from_utf8(body[..40].to_vec()).unwrap();
    assert_eq!(returned_id, fs_id, "returned fs_id must match requested");

    let size = u32::from_be_bytes([body[40], body[41], body[42], body[43]]) as usize;
    assert_eq!(
        size,
        compressed.len(),
        "size in header must match compressed data length"
    );

    // Decompress and verify
    let decompressed = pack_fs::decompress_fs_data(&body[44..44 + size]).unwrap();
    let json_val: serde_json::Value = serde_json::from_slice(&decompressed).unwrap();
    assert_eq!(json_val["type"].as_i64().unwrap(), 1);
}

/// Regression: check-blocks and check-fs must accept JSON arrays as seaf-daemon
/// sends them (Content-Type may be missing or application/x-www-form-urlencoded).
#[tokio::test]
async fn test_regression_check_endpoints_accept_json_array() {
    let (server, _api_token, repo_id, sync_token) = setup_repo().await;
    let client = server.client();

    // check-fs with JSON array
    let _existing = random_hex_id();
    let file_data = make_file_fs_data(vec![random_hex_id()], 64);
    let fs_id = file_data.compute_fs_id();
    let json = file_data.to_compact_json();
    let compressed = pack_fs::compress_fs_data(json.as_bytes()).unwrap();

    let mut packed = Vec::new();
    packed.extend_from_slice(fs_id.as_bytes());
    packed.extend_from_slice(&(compressed.len() as u32).to_be_bytes());
    packed.extend_from_slice(&compressed);
    client.recv_fs(&sync_token, &repo_id, packed).await;

    // Test check-fs accepts JSON array (what seaf-daemon sends)
    let resp = client.check_fs(&sync_token, &repo_id, &[&fs_id]).await;
    assert_eq!(resp.status(), 200);

    // Test check-blocks accepts JSON array
    let block_id = random_hex_id();
    let resp = client
        .put_block(&sync_token, &repo_id, &block_id, vec![0u8; 16])
        .await;
    assert_eq!(resp.status(), 200);

    let resp = client
        .check_blocks(&sync_token, &repo_id, &[&block_id])
        .await;
    assert_eq!(resp.status(), 200);
}

// ============================================================
// T3: resolve_fs_id path resolution tests (Phase 2)
// ============================================================

/// resolve_fs_id with path="/" must return the root fs_id directly.
#[tokio::test]
async fn test_resolve_fs_id_root_path() {
    let (server, _api_token, repo_id, sync_token) = setup_repo().await;
    let client = server.client();

    // Upload a file to create a real FS tree
    let file_data = make_file_fs_data(vec![random_hex_id()], 64);
    let file_fs_id = file_data.compute_fs_id();
    let file_json = file_data.to_compact_json();
    let file_compressed = pack_fs::compress_fs_data(file_json.as_bytes()).unwrap();

    let root_dir = make_dir_fs_data(vec![DirEntryData {
        id: file_fs_id.clone(),
        mode: 33188,
        modifier: "test@example.com".to_string(),
        mtime: chrono::Utc::now().timestamp(),
        name: "f.txt".to_string(),
        size: 64,
    }]);
    let root_fs_id = root_dir.compute_fs_id();
    let root_json = root_dir.to_compact_json();
    let root_compressed = pack_fs::compress_fs_data(root_json.as_bytes()).unwrap();

    // Upload via recv-fs
    let mut packed = Vec::new();
    packed.extend_from_slice(file_fs_id.as_bytes());
    packed.extend_from_slice(&(file_compressed.len() as u32).to_be_bytes());
    packed.extend_from_slice(&file_compressed);
    packed.extend_from_slice(root_fs_id.as_bytes());
    packed.extend_from_slice(&(root_compressed.len() as u32).to_be_bytes());
    packed.extend_from_slice(&root_compressed);
    let resp = client.recv_fs(&sync_token, &repo_id, packed).await;
    assert_eq!(resp.status(), 200);

    push_commit(&client, &sync_token, &repo_id, &root_fs_id, None).await;

    // Test resolve_fs_id directly
    let result =
        nanofile::storage::resolve_fs_id(server.db.as_ref(), &repo_id, &root_fs_id, "/", None)
            .await
            .unwrap();
    assert_eq!(result, root_fs_id, "root path must resolve to root_fs_id");

    let result_empty =
        nanofile::storage::resolve_fs_id(server.db.as_ref(), &repo_id, &root_fs_id, "", None)
            .await
            .unwrap();
    assert_eq!(
        result_empty, root_fs_id,
        "empty path must resolve to root_fs_id"
    );
}

/// resolve_fs_id with a nested path must return the correct child fs_id.
#[tokio::test]
async fn test_resolve_fs_id_deep_path() {
    let (server, _api_token, repo_id, sync_token) = setup_repo().await;
    let client = server.client();

    // Build: root -> sub/ -> nested.txt
    let file_data = make_file_fs_data(vec![random_hex_id()], 32);
    let file_fs_id = file_data.compute_fs_id();
    let file_json = file_data.to_compact_json();
    let file_compressed = pack_fs::compress_fs_data(file_json.as_bytes()).unwrap();

    let sub_dir = make_dir_fs_data(vec![DirEntryData {
        id: file_fs_id.clone(),
        mode: 33188,
        modifier: "test@example.com".to_string(),
        mtime: chrono::Utc::now().timestamp(),
        name: "nested.txt".to_string(),
        size: 32,
    }]);
    let sub_fs_id = sub_dir.compute_fs_id();
    let sub_json = sub_dir.to_compact_json();
    let sub_compressed = pack_fs::compress_fs_data(sub_json.as_bytes()).unwrap();

    let root_dir = make_dir_fs_data(vec![DirEntryData {
        id: sub_fs_id.clone(),
        mode: nanofile::serialization::S_IFDIR,
        modifier: "test@example.com".to_string(),
        mtime: chrono::Utc::now().timestamp(),
        name: "sub".to_string(),
        size: 0,
    }]);
    let root_fs_id = root_dir.compute_fs_id();
    let root_json = root_dir.to_compact_json();
    let root_compressed = pack_fs::compress_fs_data(root_json.as_bytes()).unwrap();

    // Upload via recv-fs
    let mut packed = Vec::new();
    packed.extend_from_slice(file_fs_id.as_bytes());
    packed.extend_from_slice(&(file_compressed.len() as u32).to_be_bytes());
    packed.extend_from_slice(&file_compressed);
    packed.extend_from_slice(sub_fs_id.as_bytes());
    packed.extend_from_slice(&(sub_compressed.len() as u32).to_be_bytes());
    packed.extend_from_slice(&sub_compressed);
    packed.extend_from_slice(root_fs_id.as_bytes());
    packed.extend_from_slice(&(root_compressed.len() as u32).to_be_bytes());
    packed.extend_from_slice(&root_compressed);
    let resp = client.recv_fs(&sync_token, &repo_id, packed).await;
    assert_eq!(resp.status(), 200);

    push_commit(&client, &sync_token, &repo_id, &root_fs_id, None).await;

    // Resolve /sub -> should get sub_fs_id
    let result =
        nanofile::storage::resolve_fs_id(server.db.as_ref(), &repo_id, &root_fs_id, "/sub", None)
            .await
            .unwrap();
    assert_eq!(result, sub_fs_id, "/sub must resolve to sub_fs_id");

    // Resolve /sub/nested.txt -> should get file_fs_id
    let result = nanofile::storage::resolve_fs_id(
        server.db.as_ref(),
        &repo_id,
        &root_fs_id,
        "/sub/nested.txt",
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        result, file_fs_id,
        "/sub/nested.txt must resolve to file_fs_id"
    );
}

/// resolve_fs_id with a non-existent segment must return an error.
#[tokio::test]
async fn test_resolve_fs_id_nonexistent_segment() {
    let (server, _api_token, repo_id, sync_token) = setup_repo().await;

    let root_dir = make_dir_fs_data(vec![]);
    let root_fs_id = root_dir.compute_fs_id();
    let root_json = root_dir.to_compact_json();
    let root_compressed = pack_fs::compress_fs_data(root_json.as_bytes()).unwrap();

    let mut packed = Vec::new();
    packed.extend_from_slice(root_fs_id.as_bytes());
    packed.extend_from_slice(&(root_compressed.len() as u32).to_be_bytes());
    packed.extend_from_slice(&root_compressed);
    let resp = server.client().recv_fs(&sync_token, &repo_id, packed).await;
    assert_eq!(resp.status(), 200);

    push_commit(&server.client(), &sync_token, &repo_id, &root_fs_id, None).await;

    let result = nanofile::storage::resolve_fs_id(
        server.db.as_ref(),
        &repo_id,
        &root_fs_id,
        "/nonexistent",
        None,
    )
    .await;
    assert!(result.is_err(), "non-existent path must return error");
}

/// read_fs_dir_data with an invalid fs_id must return an error.
#[tokio::test]
async fn test_read_fs_dir_data_invalid_fs_id() {
    let (server, _api_token, repo_id, _sync_token) = setup_repo().await;

    let result = nanofile::storage::read_fs_dir_data(
        server.db.as_ref(),
        &repo_id,
        "ffffffffffffffffffffffffffffffffffffffff",
    )
    .await;
    assert!(
        result.is_err(),
        "reading non-existent fs_id must return error"
    );
}

/// The zero hash is a sentinel for empty/incomplete directories —
/// read_fs_dir_data must return an empty FsDirData, not an error.
#[tokio::test]
async fn test_read_fs_dir_data_zero_hash_is_empty_dir() {
    let (server, _api_token, repo_id, _sync_token) = setup_repo().await;

    let result = nanofile::storage::read_fs_dir_data(
        server.db.as_ref(),
        &repo_id,
        "0000000000000000000000000000000000000000",
    )
    .await;
    assert!(
        result.is_ok(),
        "zero hash must return Ok(empty_dir), got: {:?}",
        result
    );
    let dir = result.unwrap();
    assert!(dir.dirents.is_empty(), "zero-hash directory must be empty");
    assert_eq!(dir.obj_type, 3, "zero-hash must be dir type");
}
