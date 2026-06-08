mod common;

use common::TestFixture;

#[tokio::test]
async fn test_server_info_authenticated() {
    let f = TestFixture::new().await;

    let resp = f.client.get("/api2/server-info/", Some(&f.api_token)).await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["version"], "8.0.0");
    assert_eq!(body["encrypted_library_version"], 3);

    let features = body["features"].as_array().unwrap();
    assert!(!features.is_empty(), "features should not be empty");
    assert!(features.iter().any(|f| f == "seafile-basic"));
}

#[tokio::test]
async fn test_server_info_unauthorized() {
    let server = common::TestServer::start().await;
    let client = server.client();

    let resp = client.get("/api2/server-info/", None).await;
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_server_info_features_include_lock_and_tag() {
    let f = TestFixture::new().await;

    let resp = f.client.get("/api2/server-info/", Some(&f.api_token)).await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let features = body["features"].as_array().unwrap();

    // Mobile clients expect these features
    assert!(
        features.iter().any(|f| f == "file_lock"),
        "file_lock feature missing"
    );
    assert!(
        features.iter().any(|f| f == "file_tag"),
        "file_tag feature missing"
    );
    assert!(
        features.iter().any(|f| f == "thumbnail"),
        "thumbnail feature missing"
    );
    assert!(
        features.iter().any(|f| f == "search"),
        "search feature missing"
    );
}

#[tokio::test]
async fn test_ping_at_api2_ping() {
    let f = TestFixture::new().await;

    // /api2/ping/ should also work (alias for /api2/auth/ping/)
    let resp = f.client.get("/api2/ping/", Some(&f.api_token)).await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["email"], f.email);
}

#[tokio::test]
async fn test_ping_at_api2_auth_ping_still_works() {
    let f = TestFixture::new().await;

    let resp = f.client.get("/api2/auth/ping/", Some(&f.api_token)).await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["email"], f.email);
}
