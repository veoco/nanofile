mod common;

use base::common::CommitData;
use common::{TestFixture, TestServer, create_test_user, get_sync_token};

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
        salt: None,
        pwd_hash: None,
        pwd_hash_algo: None,
        pwd_hash_params: None,
        key: None,
        version: 1,
        conflict: None,
        new_merge: None,
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
        salt: None,
        pwd_hash: None,
        pwd_hash_algo: None,
        pwd_hash_params: None,
        key: None,
        version: 1,
        conflict: None,
        new_merge: None,
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
        salt: None,
        pwd_hash: None,
        pwd_hash_algo: None,
        pwd_hash_params: None,
        key: Some("def456".to_string()),
        version: 1,
        conflict: None,
        new_merge: None,
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
        salt: None,
        pwd_hash: None,
        pwd_hash_algo: None,
        pwd_hash_params: None,
        key: None,
        version: 2,
        conflict: None,
        new_merge: None,
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
        salt: None,
        pwd_hash: None,
        pwd_hash_algo: None,
        pwd_hash_params: None,
        key: None,
        version: 1,
        conflict: None,
        new_merge: None,
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

/// Golden vector captured with an independent driver that implements the
/// documented input order (no upstream source is embedded). The driver ran on
/// this exact input:
/// ```text
/// root_id      = 1111111111111111111111111111111111111111
/// creator      = 2222222222222222222222222222222222222222
/// creator_name = user@example.com
/// description  = "add file"
/// ctime        = 1700000000
/// => 9044efa20f6c8b835ffbc2ced9a989e666923e18
/// ```
/// The NUL terminators after every string are part of the hash: the same fields
/// without them hash to
/// `b5cbc1b1693a0a9f9d376bed9b500dcd6e3f86e4`, which is what nanofile used to
/// produce (and which no official client would ever produce).
#[test]
fn test_compute_commit_id_matches_golden_vector() {
    let commit = CommitData {
        commit_id: String::new(),
        repo_id: "cfcab3e0-9eb4-4c4f-92d0-87db2cd8290d".to_string(),
        root_id: "1".repeat(40),
        creator_name: "user@example.com".to_string(),
        creator: "2".repeat(40),
        description: "add file".to_string(),
        ctime: 1700000000,
        parent_id: None,
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

    assert_eq!(
        server::domain::commit::compute_commit_id(&commit),
        "9044efa20f6c8b835ffbc2ced9a989e666923e18"
    );

    // An absent (empty) creator_name is the NULL case upstream skips outright
    // (upstream reference for the same input with `creator_name = NULL`:
    // `d716fcdd624f0b4ad8d8bcd958ea0912fd223f7b`).
    let mut no_name = commit.clone();
    no_name.creator_name = String::new();
    assert_eq!(
        server::domain::commit::compute_commit_id(&no_name),
        "d716fcdd624f0b4ad8d8bcd958ea0912fd223f7b"
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
        salt: None,
        pwd_hash: None,
        pwd_hash_algo: None,
        pwd_hash_params: None,
        key: None,
        version: 1,
        conflict: None,
        new_merge: None,
    };

    let json_str = serde_json::to_string(&commit_data).unwrap();
    let (commit_id, json_str) = common::stamp_commit_id(&json_str);

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

    let resp = client.get_head_commit(&sync_token, &repo_id).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let zero_sentinel = "0000000000000000000000000000000000000000";
    assert_eq!(body["head_commit_id"].as_str().unwrap(), zero_sentinel);

    let commit_id = random_hex_id();
    let now = chrono::Utc::now().timestamp();
    let commit_data = serde_json::json!({
        "commit_id": commit_id,
        "repo_id": repo_id,
        // An empty repo's root is the EMPTY_SHA1 sentinel (no object needed);
        // using a random id here would (correctly) fail root validation at
        // update_branch, which is not what this test exercises.
        "root_id": zero_sentinel,
        "creator_name": "test@example.com",
        "creator": zero_sentinel,
        "description": "first commit from client",
        "ctime": now,
        "parent_id": zero_sentinel,
        "second_parent_id": null,
        "version": 1,
    });

    let json_str = serde_json::to_string(&commit_data).unwrap();
    let (commit_id, json_str) = common::stamp_commit_id(&json_str);
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

    let resp = client.get_head_commit(&sync_token, &repo_id).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["head_commit_id"].as_str().unwrap(), commit_id);

    // A stale commit whose parent_id doesn't chain from the current HEAD is no
    // longer rejected: the server merges it (upstream's
    // `fast_forward_or_merge()`), so HEAD becomes a merge commit with the stale
    // commit as its second parent and `new_merge` set.
    let commit_id2 = random_hex_id();
    let commit_data2 = serde_json::json!({
        "commit_id": commit_id2,
        "repo_id": repo_id,
        "root_id": zero_sentinel,
        "creator_name": "test@example.com",
        "creator": zero_sentinel,
        "description": "stale commit",
        "ctime": now,
        "parent_id": zero_sentinel,  // doesn't match current HEAD (commit_id)
        "second_parent_id": null,
        "version": 1,
    });

    let json_str2 = serde_json::to_string(&commit_data2).unwrap();
    let (commit_id2, json_str2) = common::stamp_commit_id(&json_str2);
    let resp = client
        .put_commit(&sync_token, &repo_id, &commit_id2, json_str2.into_bytes())
        .await;
    assert_eq!(resp.status(), 200);

    let resp = client
        .update_branch(&sync_token, &repo_id, &commit_id2)
        .await;
    assert_eq!(resp.status(), 200);

    let resp = client.get_head_commit(&sync_token, &repo_id).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let merged_head = body["head_commit_id"].as_str().unwrap().to_string();
    assert_ne!(merged_head, commit_id2, "HEAD must be the merge commit");
    assert_ne!(merged_head, commit_id, "HEAD must be the merge commit");

    let resp = client.get_commit(&sync_token, &repo_id, &merged_head).await;
    assert_eq!(resp.status(), 200);
    let merged: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(merged["parent_id"].as_str().unwrap(), commit_id);
    assert_eq!(merged["second_parent_id"].as_str().unwrap(), commit_id2);
    assert_eq!(merged["new_merge"].as_i64().unwrap(), 1);
    assert_eq!(
        merged["description"].as_str().unwrap(),
        "Auto merge by system"
    );
    // A merge of three empty trees has no conflict.
    assert!(merged.get("conflict").is_none() || merged["conflict"].is_null());
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

/// A commit ready to upload: `commit_id` is left for the caller to fill in.
fn commit_for(repo_id: &str, description: &str) -> CommitData {
    CommitData {
        commit_id: String::new(),
        repo_id: repo_id.to_string(),
        root_id: "0".repeat(40),
        creator_name: "test@example.com".to_string(),
        creator: "0".repeat(40),
        description: description.to_string(),
        ctime: chrono::Utc::now().timestamp(),
        parent_id: None,
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
    }
}

/// The commit id is content-addressed, so a body whose id is not the hash of its own fields must
/// be refused: other clients dedupe and skip history by that id.
#[tokio::test]
async fn test_put_commit_rejects_an_id_that_is_not_the_hash() {
    let f = TestFixture::new().await;

    let mut commit = commit_for(&f.repo_id, "fabricated id");
    commit.commit_id = random_hex_id();
    let body = serde_json::to_string(&commit).unwrap();

    let resp = f
        .client
        .put_commit(
            &f.sync_token,
            &f.repo_id,
            &commit.commit_id,
            body.into_bytes(),
        )
        .await;
    assert_eq!(resp.status(), 400, "a fabricated id must be refused");
}

/// A commit uploaded to another commit's URL is refused as well: the path names the object.
#[tokio::test]
async fn test_put_commit_rejects_an_id_that_does_not_match_the_path() {
    let f = TestFixture::new().await;

    let mut commit = commit_for(&f.repo_id, "path mismatch");
    commit.commit_id = server::domain::commit::compute_commit_id(&commit);
    let body = serde_json::to_string(&commit).unwrap();

    let resp = f
        .client
        .put_commit(
            &f.sync_token,
            &f.repo_id,
            &random_hex_id(),
            body.into_bytes(),
        )
        .await;
    assert_eq!(
        resp.status(),
        400,
        "the URL must name the commit the body describes"
    );
}

/// A client that sends `creator_name: ""` hashed a NUL terminator for it; one that omits the field
/// hashed nothing. A parsed `CommitData` cannot tell them apart, so both spellings of the same
/// commit must upload.
#[tokio::test]
async fn test_put_commit_accepts_both_empty_creator_name_spellings() {
    let f = TestFixture::new().await;

    for (index, include_empty_name) in [false, true].into_iter().enumerate() {
        let mut commit = commit_for(&f.repo_id, &format!("empty creator name {index}"));
        commit.creator_name = String::new();
        let id =
            server::domain::commit::compute_commit_id_with_empty_name(&commit, include_empty_name);
        commit.commit_id = id.clone();

        // `CommitData` skips an empty name when serializing, which the server cannot parse back
        // (the field has no `default`), so spell it explicitly the way a client does.
        let mut value = serde_json::to_value(&commit).unwrap();
        value["creator_name"] = serde_json::Value::String(String::new());
        let body = serde_json::to_string(&value).unwrap();

        let resp = f
            .client
            .put_commit(&f.sync_token, &f.repo_id, &id, body.into_bytes())
            .await;
        assert_eq!(
            resp.status(),
            200,
            "the {} spelling of an empty creator_name must be accepted",
            if include_empty_name {
                "hashed"
            } else {
                "omitted"
            }
        );
    }
}

/// Regression: a client upload advances the library's modification time too. The
/// value is the uploaded commit's `ctime`, matching the server branch-update hook.
#[tokio::test]
async fn test_update_branch_bumps_repo_mtime_to_commit_time() {
    let f = TestFixture::new().await;
    let before = f.repo_mtime().await;

    // `updated_at` is second-resolution, so make creation and upload distinct seconds.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;

    let zero = "0".repeat(40);
    let commit_id = random_hex_id();
    let ctime = chrono::Utc::now().timestamp();
    let commit_data = serde_json::json!({
        "commit_id": commit_id,
        "repo_id": f.repo_id,
        // An empty repo's root is the EMPTY_SHA1 sentinel; no object is needed.
        "root_id": zero,
        "creator_name": "test@example.com",
        "creator": zero,
        "description": "sync mtime test",
        "ctime": ctime,
        "parent_id": zero,
        "second_parent_id": null,
        "version": 1,
    });

    let body = serde_json::to_string(&commit_data).unwrap();
    let (commit_id, body) = common::stamp_commit_id(&body);
    let resp = f
        .client
        .put_commit(&f.sync_token, &f.repo_id, &commit_id, body.into_bytes())
        .await;
    assert_eq!(resp.status(), 200, "put_commit failed");

    let resp = f
        .client
        .update_branch(&f.sync_token, &f.repo_id, &commit_id)
        .await;
    assert_eq!(resp.status(), 200, "update_branch failed");

    let after = f.repo_mtime().await;
    assert!(
        after > before,
        "client upload must advance the library mtime: {before} -> {after}"
    );
    assert_eq!(
        after, ctime,
        "mtime must be the uploaded commit's ctime, not the wall clock"
    );
}
