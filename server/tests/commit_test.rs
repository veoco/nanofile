mod common;

use base::common::CommitData;
use common::{TestServer, create_test_user, get_sync_token};

fn random_hex_id() -> String {
    use sha1::{Digest, Sha1};
    let mut hasher = Sha1::new();
    hasher.update(rand::random::<[u8; 32]>());
    hex::encode(hasher.finalize())
}

#[tokio::test]
async fn test_commit_serialization_types() {
    let commit = CommitData {
        commit_id: random_hex_id(),
        repo_id: "test-repo-id".to_string(),
        root_id: random_hex_id(),
        creator_name: "test@example.com".to_string(),
        creator: "0000000000000000000000000000000000000000".to_string(),
        description: "test commit".to_string(),
        ctime: 1700000000,
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

    let json = serde_json::to_string(&commit).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed["ctime"].as_i64().unwrap(), 1700000000);
    assert!(parsed["ctime"].is_number());
    assert!(parsed.get("version").is_some());
    assert_eq!(parsed["version"].as_i64().unwrap(), 1);
}

#[tokio::test]
async fn test_commit_serialization_null() {
    let commit = CommitData {
        commit_id: random_hex_id(),
        repo_id: "test-repo-id".to_string(),
        root_id: random_hex_id(),
        creator_name: "test@example.com".to_string(),
        creator: "0000000000000000000000000000000000000000".to_string(),
        description: "test commit".to_string(),
        ctime: 1700000000,
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

    let json = serde_json::to_string(&commit).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

    assert!(parsed["parent_id"].is_null());
    assert!(parsed["second_parent_id"].is_null());
    assert!(parsed.get("repo_name").is_none());
    assert!(parsed.get("encrypted").is_none());
}

#[tokio::test]
async fn test_commit_serialization_optional_fields() {
    let commit = CommitData {
        commit_id: random_hex_id(),
        repo_id: "test-repo-id".to_string(),
        root_id: random_hex_id(),
        creator_name: "test@example.com".to_string(),
        creator: "0000000000000000000000000000000000000000".to_string(),
        description: "test commit".to_string(),
        ctime: 1700000000,
        parent_id: Some(random_hex_id()),
        second_parent_id: Some(random_hex_id()),
        repo_name: Some("My Library".to_string()),
        repo_desc: Some("A test library".to_string()),
        repo_category: None,
        encrypted: Some("false".to_string()),
        enc_version: Some(2),
        magic: Some("abc123".to_string()),
        key: Some("def456".to_string()),
        version: 1,
    };

    let json = serde_json::to_string(&commit).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

    assert!(!parsed["parent_id"].is_null());
    assert!(!parsed["second_parent_id"].is_null());
    assert_eq!(parsed["repo_name"].as_str().unwrap(), "My Library");
    assert_eq!(parsed["repo_desc"].as_str().unwrap(), "A test library");
    assert!(parsed.get("repo_category").is_none());
    assert_eq!(parsed["encrypted"].as_str().unwrap(), "false");
    assert_eq!(parsed["enc_version"].as_i64().unwrap(), 2);
    assert_eq!(parsed["magic"].as_str().unwrap(), "abc123");
    assert_eq!(parsed["key"].as_str().unwrap(), "def456");
}

#[tokio::test]
async fn test_commit_roundtrip() {
    let commit = CommitData {
        commit_id: random_hex_id(),
        repo_id: "test-repo-id".to_string(),
        root_id: random_hex_id(),
        creator_name: "test@example.com".to_string(),
        creator: "0000000000000000000000000000000000000000".to_string(),
        description: "test commit".to_string(),
        ctime: 1700000000,
        parent_id: Some(random_hex_id()),
        second_parent_id: None,
        repo_name: Some("My Library".to_string()),
        repo_desc: None,
        repo_category: None,
        encrypted: None,
        enc_version: None,
        magic: None,
        key: None,
        version: 2,
    };

    let json = serde_json::to_string(&commit).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

    let roundtripped: CommitData = serde_json::from_value(parsed).unwrap();

    assert_eq!(roundtripped.commit_id, commit.commit_id);
    assert_eq!(roundtripped.repo_id, commit.repo_id);
    assert_eq!(roundtripped.root_id, commit.root_id);
    assert_eq!(roundtripped.ctime, commit.ctime);
    assert_eq!(roundtripped.version, 2);
    assert_eq!(roundtripped.parent_id, commit.parent_id);
    assert_eq!(roundtripped.second_parent_id, commit.second_parent_id);
}

#[tokio::test]
async fn test_compute_commit_id() {
    let commit = CommitData {
        commit_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        repo_id: "test-repo-id".to_string(),
        root_id: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
        creator_name: "test@example.com".to_string(),
        creator: "0000000000000000000000000000000000000000".to_string(),
        description: "test commit".to_string(),
        ctime: 1700000000,
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

    let commit_id = server::domain::commit::compute_commit_id(&commit);
    assert_eq!(commit_id.len(), 40);

    let commit_id2 = server::domain::commit::compute_commit_id(&commit);
    assert_eq!(commit_id, commit_id2, "commit_id should be deterministic");

    let mut commit2 = commit.clone();
    commit2.description = "different description".to_string();
    let commit_id3 = server::domain::commit::compute_commit_id(&commit2);
    assert_ne!(
        commit_id, commit_id3,
        "different data should produce different commit_id"
    );
}

#[tokio::test]
async fn test_put_get_commit_raw_json() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let api_token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, api_token, "Commit Test").await;
    let sync_token = get_sync_token(&client, api_token, &repo_id).await;

    let commit_id = random_hex_id();
    let root_id = random_hex_id();
    let now = chrono::Utc::now().timestamp();

    let commit_data = CommitData {
        commit_id: commit_id.clone(),
        repo_id: repo_id.clone(),
        root_id: root_id.clone(),
        creator_name: "test@example.com".to_string(),
        creator: "0000000000000000000000000000000000000000".to_string(),
        description: "test commit".to_string(),
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

    let resp = client.get_commit(&sync_token, &repo_id, &commit_id).await;
    assert_eq!(resp.status(), 200);
    let body = resp.bytes().await.unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(parsed["commit_id"].as_str().unwrap(), commit_id);
    assert_eq!(parsed["root_id"].as_str().unwrap(), root_id);
    assert_eq!(parsed["ctime"].as_i64().unwrap(), now);
}

#[tokio::test]
async fn test_put_commit_validates_repo_id() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let api_token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, api_token, "Commit Test").await;
    let sync_token = get_sync_token(&client, api_token, &repo_id).await;

    let commit_id = random_hex_id();

    let commit_data = serde_json::json!({
        "commit_id": commit_id,
        "repo_id": "wrong-repo-id",
        "root_id": random_hex_id(),
        "creator_name": "test@example.com",
        "creator": "0000000000000000000000000000000000000000",
        "description": "test commit",
        "ctime": chrono::Utc::now().timestamp(),
        "parent_id": null,
        "second_parent_id": null,
        "version": 1
    });

    let json_str = serde_json::to_string(&commit_data).unwrap();
    let resp = client
        .put_commit(&sync_token, &repo_id, &commit_id, json_str.into_bytes())
        .await;
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn test_update_branch_accepts_zero_sentinel_parent() {
    // Regression test: repos created via REST API have head_commit_id = NULL.
    // get_head_commit() returns "0000..." sentinel to the client. The seafile
    // daemon may set parent_id to this sentinel. update_branch must accept it
    // rather than returning 409 (Some("0000...") != None).
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let api_token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, api_token, "Regression Test").await;
    let sync_token = get_sync_token(&client, api_token, &repo_id).await;

    // Verify the repo has no HEAD commit (returns zero sentinel)
    let resp = client.get_head_commit(&sync_token, &repo_id).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let zero_sentinel = "0000000000000000000000000000000000000000";
    assert_eq!(body["head_commit_id"].as_str().unwrap(), zero_sentinel);

    // Create a commit with parent_id = zero sentinel, mimicking the actual
    // seafile client behavior when syncing an empty repo.
    let commit_id = random_hex_id();
    let now = chrono::Utc::now().timestamp();
    let commit_data = serde_json::json!({
        "commit_id": commit_id,
        "repo_id": repo_id,
        "root_id": random_hex_id(),
        "creator_name": "test@example.com",
        "creator": zero_sentinel,
        "description": "first commit from client",
        "ctime": now,
        "parent_id": zero_sentinel,
        "second_parent_id": null,
        "version": 1,
    });

    let json_str = serde_json::to_string(&commit_data).unwrap();
    let resp = client
        .put_commit(&sync_token, &repo_id, &commit_id, json_str.into_bytes())
        .await;
    assert_eq!(resp.status(), 200);

    // Before fix: 409 Conflict (parent_id Some("0000...") != current_head None)
    // After fix: 200 (skip parent check when current_head is None)
    let resp = client
        .update_branch(&sync_token, &repo_id, &commit_id)
        .await;
    assert_eq!(resp.status(), 200);

    // Verify HEAD was updated
    let resp = client.get_head_commit(&sync_token, &repo_id).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["head_commit_id"].as_str().unwrap(), commit_id);

    // Verify subsequent 409 conflict still works: try to set a stale commit
    // whose parent_id doesn't chain from the new current HEAD.
    let commit_id2 = random_hex_id();
    let commit_data2 = serde_json::json!({
        "commit_id": commit_id2,
        "repo_id": repo_id,
        "root_id": random_hex_id(),
        "creator_name": "test@example.com",
        "creator": zero_sentinel,
        "description": "stale commit",
        "ctime": now,
        "parent_id": zero_sentinel,  // doesn't match current HEAD (commit_id)
        "second_parent_id": null,
        "version": 1,
    });

    let json_str2 = serde_json::to_string(&commit_data2).unwrap();
    let resp = client
        .put_commit(&sync_token, &repo_id, &commit_id2, json_str2.into_bytes())
        .await;
    assert_eq!(resp.status(), 200);

    let resp = client
        .update_branch(&sync_token, &repo_id, &commit_id2)
        .await;
    assert_eq!(resp.status(), 409);
}

#[tokio::test]
async fn test_update_branch_verifies_commit() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let api_token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, api_token, "Branch Test").await;
    let sync_token = get_sync_token(&client, api_token, &repo_id).await;

    let fake_commit_id = random_hex_id();

    let resp = client
        .update_branch(&sync_token, &repo_id, &fake_commit_id)
        .await;
    // Seafile returns 500 (EVHTP_RES_SERVERR) when the commit does not
    // exist — matching put_update_branch_cb behavior.
    assert_eq!(resp.status(), 500);
}
