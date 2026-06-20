mod common;

use common::TestFixture;

/// B.6.1 — GET /api2/groups/
#[tokio::test]
async fn test_groups_empty() {
    let f = TestFixture::new().await;
    let resp = f.client.get("/api2/groups/", Some(&f.api_token)).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_groups_unauthorized() {
    let server = common::TestServer::start().await;
    let client = server.client();
    let resp = client.get("/api2/groups/", None).await;
    assert_eq!(resp.status(), 401);
}

/// B.6.2 — GET /api2/groupandcontacts/
#[tokio::test]
async fn test_groupandcontacts_empty() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .get("/api2/groupandcontacts/", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["groups"].as_array().unwrap().is_empty());
    assert!(body["contacts"].as_array().unwrap().is_empty());
}

/// B.6.3 — GET /api2/search-user/?q=
#[tokio::test]
async fn test_search_user_found() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .get("/api2/search-user/?q=test", Some(&f.api_token))
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(!body.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_search_user_not_found() {
    let f = TestFixture::new().await;
    let resp = f
        .client
        .get(
            "/api2/search-user/?q=nonexistent_user_xyz",
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body.as_array().unwrap().len(), 0);
}
