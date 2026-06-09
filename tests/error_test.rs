mod common;

use common::{TestServer, create_test_user, get_sync_token};

fn random_hex_id() -> String {
    use sha1::{Digest, Sha1};
    let mut hasher = Sha1::new();
    hasher.update(rand::random::<[u8; 32]>());
    hex::encode(hasher.finalize())
}

#[tokio::test]
async fn test_update_branch_invalid_commit() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let api_token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, api_token, "Error Test").await;
    let sync_token = get_sync_token(&client, api_token, &repo_id).await;

    let resp = client
        .update_branch(&sync_token, &repo_id, &random_hex_id())
        .await;
    // Seafile returns 500 (EVHTP_RES_SERVERR) when commit not found.
    assert_eq!(resp.status(), 500);
}

#[tokio::test]
async fn test_update_branch_no_auth() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let api_token = body["token"].as_str().unwrap().to_string();

    let repo_id = common::create_test_repo(&client, &api_token, "Error Test").await;

    let resp = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .put(format!(
            "{}/seafhttp/repo/{}/commit/HEAD?head={}",
            server.base_url,
            repo_id,
            random_hex_id()
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_recv_fs_no_auth() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let api_token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, api_token, "Error Test").await;

    let resp = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!(
            "{}/seafhttp/repo/{}/recv-fs/",
            server.base_url, repo_id
        ))
        .body(vec![0u8; 100])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_pack_fs_no_auth() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let api_token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, api_token, "Error Test").await;

    let resp = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!(
            "{}/seafhttp/repo/{}/pack-fs/",
            server.base_url, repo_id
        ))
        .json(&serde_json::json!(["some_id"]))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_check_blocks_no_auth() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let api_token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, api_token, "Error Test").await;

    let resp = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!(
            "{}/seafhttp/repo/{}/check-blocks/",
            server.base_url, repo_id
        ))
        .json(&serde_json::json!(["block_id"]))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_get_commit_not_found() {
    let server = TestServer::start().await;
    let client = server.client();

    create_test_user(server.db.as_ref(), "test@example.com", "password123").await;
    let resp = client.login("test@example.com", "password123").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let api_token = body["token"].as_str().unwrap();

    let repo_id = common::create_test_repo(&client, api_token, "Error Test").await;
    let sync_token = get_sync_token(&client, api_token, &repo_id).await;

    let resp = client
        .get_commit(&sync_token, &repo_id, &random_hex_id())
        .await;
    assert_eq!(resp.status(), 404);
}
