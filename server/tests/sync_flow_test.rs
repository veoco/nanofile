mod common;

use base::common::CommitData;
use base::common::{DirEntryData, FsDirData, FsFileData};
use common::{TestServer, create_test_user, get_sync_token};
use infra::serialization::pack_fs;

fn random_hex_id() -> String {
    use sha1::{Digest, Sha1};
    let mut hasher = Sha1::new();
    hasher.update(rand::random::<[u8; 32]>());
    hex::encode(hasher.finalize())
}

async fn setup_repo() -> (TestServer, String, String, String) {
    let server = TestServer::start().await;
    let client = server.client();
    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let api_token = body["token"].as_str().unwrap().to_string();
    let repo_id = common::create_test_repo(&client, &api_token, "Sync Flow Test").await;
    let sync_token = get_sync_token(&client, &api_token, &repo_id).await;
    (server, api_token, repo_id, sync_token)
}

#[tokio::test]
async fn test_full_upload_flow() {
    let (server, _api_token, repo_id, sync_token) = setup_repo().await;
    let client = server.client();
    use infra::crypto::fs_id::sha1_hex;

    let resp = client
        .permission_check(&sync_token, &repo_id, "upload")
        .await;
    assert_eq!(resp.status(), 200);

    let resp = client.get_head_commit(&sync_token, &repo_id).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["is_corrupted"].as_i64().unwrap(), 0);
    assert_eq!(
        body["head_commit_id"].as_str().unwrap(),
        "0000000000000000000000000000000000000000"
    );

    // Upload the block first so the block integrity check passes.
    let block_data = b"test block data content here!!!!!!";
    let file_block = sha1_hex(block_data);
    let resp = client
        .put_block(&sync_token, &repo_id, &file_block, block_data.to_vec())
        .await;
    assert_eq!(resp.status(), 200);

    let file_data = FsFileData {
        block_ids: vec![file_block.clone()],
        size: 100,
        obj_type: 1,
        version: 1,
    };
    let file_fs_id =
        infra::crypto::fs_id::sha1_hex(serde_json::to_string(&file_data).unwrap().as_bytes());
    let file_json = serde_json::to_string(&file_data).unwrap();
    let file_compressed = pack_fs::compress_fs_data(file_json.as_bytes()).unwrap();

    let root_dir = FsDirData {
        dirents: vec![DirEntryData {
            id: file_fs_id.clone(),
            mode: 33188,
            modifier: "test@example.com".to_string(),
            mtime: chrono::Utc::now().timestamp(),
            name: "test.txt".to_string(),
            size: 100,
        }],
        obj_type: 3,
        version: 1,
    };
    let root_fs_id =
        infra::crypto::fs_id::sha1_hex(serde_json::to_string(&root_dir).unwrap().as_bytes());
    let root_json = serde_json::to_string(&root_dir).unwrap();
    let root_compressed = pack_fs::compress_fs_data(root_json.as_bytes()).unwrap();

    let mut fs_pack = Vec::new();
    fs_pack.extend_from_slice(file_fs_id.as_bytes());
    fs_pack.extend_from_slice(&(file_compressed.len() as u32).to_be_bytes());
    fs_pack.extend_from_slice(&file_compressed);
    fs_pack.extend_from_slice(root_fs_id.as_bytes());
    fs_pack.extend_from_slice(&(root_compressed.len() as u32).to_be_bytes());
    fs_pack.extend_from_slice(&root_compressed);

    let resp = client.recv_fs(&sync_token, &repo_id, fs_pack).await;
    assert_eq!(resp.status(), 200);

    let resp = client
        .check_fs(&sync_token, &repo_id, &[&file_fs_id, &root_fs_id])
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let missing = body.as_array().unwrap();
    assert!(missing.is_empty());

    let block_id = sha1_hex(block_data);
    let resp = client
        .put_block(&sync_token, &repo_id, &block_id, block_data.to_vec())
        .await;
    assert_eq!(resp.status(), 200);

    let resp = client
        .check_blocks(&sync_token, &repo_id, &[&block_id])
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let missing = body.as_array().unwrap();
    assert!(missing.is_empty());

    let commit_id = random_hex_id();
    let now = chrono::Utc::now().timestamp();
    let commit_data = CommitData {
        commit_id: commit_id.clone(),
        repo_id: repo_id.clone(),
        root_id: root_fs_id.clone(),
        creator_name: "test@example.com".to_string(),
        creator: "0000000000000000000000000000000000000000".to_string(),
        description: "upload test".to_string(),
        ctime: now,
        parent_id: None,
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
    let json_str = serde_json::to_string(&commit_data).unwrap();
    let resp = client
        .put_commit(&sync_token, &repo_id, &commit_id, json_str.into_bytes())
        .await;
    assert_eq!(resp.status(), 200);

    let resp = client
        .update_branch(&sync_token, &repo_id, &commit_id)
        .await;
    assert_eq!(resp.status(), 200);

    let resp = client.get_head_commit(&sync_token, &repo_id).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["is_corrupted"].as_i64().unwrap(), 0);
    assert_eq!(body["head_commit_id"].as_str().unwrap(), commit_id);
}

#[tokio::test]
async fn test_full_download_flow() {
    let (server, api_token, repo_id, sync_token) = setup_repo().await;
    let client = server.client();

    let file_content = b"Hello Seafile Download!";
    let resp = client
        .upload_file(&api_token, &repo_id, "/", "download_test.txt", file_content)
        .await;
    assert_eq!(resp.status(), 200);

    let resp = client
        .permission_check(&sync_token, &repo_id, "download")
        .await;
    assert_eq!(resp.status(), 200);

    let resp = client.get_head_commit(&sync_token, &repo_id).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["is_corrupted"].as_i64().unwrap(), 0);
    let head_commit_id = body["head_commit_id"].as_str().unwrap().to_string();

    let resp = client
        .get_commit(&sync_token, &repo_id, &head_commit_id)
        .await;
    assert_eq!(resp.status(), 200);
    let commit_bytes = resp.bytes().await.unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&commit_bytes).unwrap();
    assert_eq!(parsed["commit_id"].as_str().unwrap(), head_commit_id);
    assert!(parsed["ctime"].is_number());

    let resp = client
        .fs_id_list(&sync_token, &repo_id, &head_commit_id)
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let fs_ids: Vec<String> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert!(!fs_ids.is_empty());

    let fs_id_refs: Vec<&str> = fs_ids.iter().map(|s| s.as_str()).collect();
    let resp = client.pack_fs(&sync_token, &repo_id, &fs_id_refs).await;
    assert_eq!(resp.status(), 200);
    let packed = resp.bytes().await.unwrap();
    assert!(!packed.is_empty());

    let entries = pack_fs::decode_pack_fs_entries(&packed).unwrap();
    assert!(!entries.is_empty());
    for (id, data) in &entries {
        assert_eq!(id.len(), 40);
        assert!(!data.is_empty());
    }

    let resp = client
        .download_file(&api_token, &repo_id, "/download_test.txt")
        .await;
    assert_eq!(resp.status(), 200);
    let downloaded = resp.bytes().await.unwrap();
    assert_eq!(downloaded.as_ref(), file_content);
}

#[tokio::test]
async fn test_incremental_upload() {
    let (server, _api_token, repo_id, sync_token) = setup_repo().await;
    let client = server.client();
    use infra::crypto::fs_id::sha1_hex;

    // Upload a real block so the block integrity check passes.
    let block_data = b"block data for incremental upload test";
    let block_id = sha1_hex(block_data);
    let resp = client
        .put_block(&sync_token, &repo_id, &block_id, block_data.to_vec())
        .await;
    assert_eq!(resp.status(), 200);

    let file_data = FsFileData {
        block_ids: vec![block_id.clone()],
        size: 100,
        obj_type: 1,
        version: 1,
    };
    let file_fs_id =
        infra::crypto::fs_id::sha1_hex(serde_json::to_string(&file_data).unwrap().as_bytes());
    let file_json = serde_json::to_string(&file_data).unwrap();
    let file_compressed = pack_fs::compress_fs_data(file_json.as_bytes()).unwrap();

    let root_dir = FsDirData {
        dirents: vec![DirEntryData {
            id: file_fs_id.clone(),
            mode: 33188,
            modifier: "test@example.com".to_string(),
            mtime: chrono::Utc::now().timestamp(),
            name: "file.txt".to_string(),
            size: 100,
        }],
        obj_type: 3,
        version: 1,
    };
    let root_fs_id =
        infra::crypto::fs_id::sha1_hex(serde_json::to_string(&root_dir).unwrap().as_bytes());
    let root_json = serde_json::to_string(&root_dir).unwrap();
    let root_compressed = pack_fs::compress_fs_data(root_json.as_bytes()).unwrap();

    let mut fs_pack = Vec::new();
    fs_pack.extend_from_slice(file_fs_id.as_bytes());
    fs_pack.extend_from_slice(&(file_compressed.len() as u32).to_be_bytes());
    fs_pack.extend_from_slice(&file_compressed);
    fs_pack.extend_from_slice(root_fs_id.as_bytes());
    fs_pack.extend_from_slice(&(root_compressed.len() as u32).to_be_bytes());
    fs_pack.extend_from_slice(&root_compressed);

    let resp = client.recv_fs(&sync_token, &repo_id, fs_pack).await;
    assert_eq!(resp.status(), 200);

    let commit_id = random_hex_id();
    let now = chrono::Utc::now().timestamp();
    let commit_data = CommitData {
        commit_id: commit_id.clone(),
        repo_id: repo_id.clone(),
        root_id: root_fs_id.clone(),
        creator_name: "test@example.com".to_string(),
        creator: "0000000000000000000000000000000000000000".to_string(),
        description: "first commit".to_string(),
        ctime: now,
        parent_id: None,
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
    let json_str = serde_json::to_string(&commit_data).unwrap();
    let resp = client
        .put_commit(&sync_token, &repo_id, &commit_id, json_str.into_bytes())
        .await;
    assert_eq!(resp.status(), 200);
    let resp = client
        .update_branch(&sync_token, &repo_id, &commit_id)
        .await;
    assert_eq!(resp.status(), 200);

    let resp = client
        .check_fs(&sync_token, &repo_id, &[&file_fs_id, &root_fs_id])
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let missing = body.as_array().unwrap();
    assert!(missing.is_empty());
}

#[tokio::test]
async fn test_incremental_download() {
    let (server, api_token, repo_id, sync_token) = setup_repo().await;
    let client = server.client();

    let resp = client
        .upload_file(&api_token, &repo_id, "/", "test.txt", b"content")
        .await;
    assert_eq!(resp.status(), 200);

    let resp = client.get_head_commit(&sync_token, &repo_id).await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let head = body["head_commit_id"].as_str().unwrap().to_string();

    let resp = client
        .fs_id_list_with_client(&sync_token, &repo_id, &head, &head)
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let fs_ids = body.as_array().unwrap();
    assert!(fs_ids.is_empty());
}

#[tokio::test]
async fn test_head_commits_multi_partial() {
    let (server, api_token, repo_id, _sync_token) = setup_repo().await;
    let client = server.client();

    let resp = client
        .upload_file(&api_token, &repo_id, "/", "test.txt", b"hello")
        .await;
    assert_eq!(resp.status(), 200);

    let fake_repo_id = random_hex_id();
    let resp = client.head_commits_multi(&[&repo_id, &fake_repo_id]).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let obj = body.as_object().unwrap();
    assert!(obj.contains_key(&repo_id));
    assert!(!obj.contains_key(&fake_repo_id));
}

#[tokio::test]
async fn test_head_commits_multi_with_real_data() {
    let (server, api_token, repo_id, _sync_token) = setup_repo().await;
    let client = server.client();

    let resp = client
        .upload_file(&api_token, &repo_id, "/", "test.txt", b"hello")
        .await;
    assert_eq!(resp.status(), 200);

    let resp = client.head_commits_multi(&[&repo_id]).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let commit_id = body[&repo_id].as_str().unwrap();
    assert_eq!(commit_id.len(), 40);
}

/// Regression test: empty directories must use EMPTY_SHA1 sentinel
/// so the seafile client's diff engine (expand_dir_added_cb) generates
/// DIR_ADDED entries. Using a real SHA1 would silently drop the entry.
#[tokio::test]
async fn test_empty_directory_sync() {
    let (server, api_token, repo_id, sync_token) = setup_repo().await;
    let client = server.client();

    // Step 1: Create an empty directory via the API
    let resp = client.create_dir(&api_token, &repo_id, "/emptydir").await;
    assert_eq!(resp.status(), 200, "create empty dir should succeed");

    // Step 2: Verify the empty directory appears in the parent dir listing
    let resp = client.list_dir(&api_token, &repo_id, "/").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let empty_dir = body
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "emptydir")
        .unwrap();
    assert_eq!(empty_dir["type"], "dir", "should be listed as directory");
    assert_eq!(empty_dir["size"], 0, "empty directory should have size 0");

    // Step 3: Create a second empty dir for the v2.1 API
    let v21_path = format!("/api/v2.1/repos/{}/dir/", repo_id);
    let resp = client
        .post_json(
            &v21_path,
            Some(&api_token),
            &serde_json::json!({"p": "/v21emptydir", "operation": "mkdir"}),
        )
        .await;
    assert_eq!(resp.status(), 200, "v2.1 create empty dir should succeed");

    // Step 4: Get the head commit — should be a real commit
    let resp = client.get_head_commit(&sync_token, &repo_id).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let head_commit_id = body["head_commit_id"].as_str().unwrap().to_string();
    assert_ne!(
        head_commit_id, "0000000000000000000000000000000000000000",
        "should have a real commit after creating dirs"
    );

    // Step 5: Verify the commit exists and has a non-EMPTY root
    let resp = client
        .get_commit(&sync_token, &repo_id, &head_commit_id)
        .await;
    assert_eq!(resp.status(), 200);
    let commit_bytes = resp.bytes().await.unwrap();
    let commit_val: serde_json::Value = serde_json::from_slice(&commit_bytes).unwrap();
    let root_id = commit_val["root_id"].as_str().unwrap().to_string();
    assert_ne!(
        root_id, "0000000000000000000000000000000000000000",
        "root should not be EMPTY_SHA1"
    );

    // Step 6: fs-id-list — must NOT contain EMPTY_SHA1
    let resp = client
        .fs_id_list(&sync_token, &repo_id, &head_commit_id)
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let fs_ids: Vec<String> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert!(!fs_ids.is_empty(), "fs-id-list should not be empty");
    assert!(
        !fs_ids.contains(&"0000000000000000000000000000000000000000".to_string()),
        "EMPTY_SHA1 must NOT appear in fs-id-list — it's a sentinel, not an fs_object"
    );

    // Step 7: pack-fs the root — verify the root's dirents contain EMPTY_SHA1
    let resp = client.pack_fs(&sync_token, &repo_id, &[&root_id]).await;
    assert_eq!(resp.status(), 200);
    let packed = resp.bytes().await.unwrap();
    let entries = pack_fs::decode_pack_fs_entries(&packed).unwrap();
    assert_eq!(entries.len(), 1, "should have one entry (the root)");
    let (packed_id, packed_data) = &entries[0];
    assert_eq!(packed_id, &root_id);
    let decompressed = pack_fs::decompress_fs_data(packed_data).unwrap();
    let dir_data: FsDirData = serde_json::from_slice(&decompressed).unwrap();
    assert_eq!(dir_data.obj_type, 3);
    let emptydir_entry = dir_data
        .dirents
        .iter()
        .find(|d| d.name == "emptydir")
        .expect("root should contain emptydir entry");
    assert_eq!(
        emptydir_entry.id, "0000000000000000000000000000000000000000",
        "empty directory dirent must use EMPTY_SHA1 id"
    );
    assert_eq!(emptydir_entry.mode & 0o170000, 0o40000, "should be S_IFDIR");
    assert_eq!(emptydir_entry.size, 0);

    let v21_entry = dir_data
        .dirents
        .iter()
        .find(|d| d.name == "v21emptydir")
        .expect("root should contain v21emptydir entry");
    assert_eq!(
        v21_entry.id, "0000000000000000000000000000000000000000",
        "v2.1 empty directory must also use EMPTY_SHA1"
    );

    // Step 8: pack-fs for EMPTY_SHA1 directly — should return nothing (no fs_object)
    let resp = client
        .pack_fs(
            &sync_token,
            &repo_id,
            &["0000000000000000000000000000000000000000"],
        )
        .await;
    assert_eq!(resp.status(), 200);
    let packed = resp.bytes().await.unwrap();
    let entries = pack_fs::decode_pack_fs_entries(&packed).unwrap();
    assert!(
        entries.is_empty(),
        "pack-fs for EMPTY_SHA1 should return no entries"
    );

    // Step 9: File detail for the empty directory should return 400 (is a dir)
    let resp = client
        .get(
            &format!("/api2/repos/{}/file/detail/?p=/emptydir", repo_id),
            Some(&api_token),
        )
        .await;
    assert_eq!(resp.status(), 400, "file detail on an empty dir should 400");

    // Step 10: Thumbnail for the empty directory should return 400
    let resp = client
        .get(
            &format!("/api2/repos/{}/thumbnail/?p=/emptydir&size=48", repo_id),
            Some(&api_token),
        )
        .await;
    assert_eq!(resp.status(), 400, "thumbnail on an empty dir should 400");

    // Step 11: Upload a file inside the empty dir — verifies commit chain works
    let resp = client
        .upload_file(&api_token, &repo_id, "/emptydir", "test.txt", b"hello")
        .await;
    assert_eq!(
        resp.status(),
        200,
        "should be able to upload file into empty dir"
    );

    // Step 12: Verify the file is accessible
    let resp = client
        .download_file(&api_token, &repo_id, "/emptydir/test.txt")
        .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.bytes().await.unwrap().as_ref(), b"hello");
}
