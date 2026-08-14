mod common;

use common::TestFixture;

#[tokio::test]
async fn test_server_info_public() {
    let server = common::TestServer::start().await;
    let client = server.client();

    // Must be accessible without authentication (matching original seahub)
    let resp = client.get("/api2/server-info/", None).await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["version"], "8.0.0");
    assert_eq!(body["encrypted_library_version"], 3);

    let features = body["features"].as_array().unwrap();
    assert!(!features.is_empty(), "features should not be empty");
    assert!(features.iter().any(|f| f == "seafile-basic"));
}

#[tokio::test]
async fn test_server_info_features_are_official() {
    let server = common::TestServer::start().await;
    let client = server.client();

    // Also accessible without auth
    let resp = client.get("/api2/server-info/", None).await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let features = body["features"].as_array().unwrap();

    // These are the feature strings the official clients actually check.
    for want in ["seafile-basic", "seafile-pro", "file-search", "wiki"] {
        assert!(features.iter().any(|f| f == want), "{want} feature missing");
    }
    // The mobile search tab keys off "file-search", not "search".
    assert!(
        features.iter().any(|f| f == "file-search"),
        "search must be advertised as file-search"
    );
}

#[tokio::test]
async fn test_ping_at_api2_ping() {
    // /api2/ping/ should be public and return "pong"
    let server = common::TestServer::start().await;
    let client = server.client();

    let resp = client.get("/api2/ping/", None).await;
    assert_eq!(resp.status(), 200);

    let body: String = resp.text().await.unwrap();
    assert_eq!(
        body, "\"pong\"",
        "public ping should return the string \"pong\""
    );
}

#[tokio::test]
async fn test_ping_at_api2_auth_ping_still_works() {
    let f = TestFixture::new().await;

    let resp = f.client.get("/api2/auth/ping/", Some(&f.api_token)).await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["email"], f.email);
}

#[tokio::test]
async fn test_ping_at_api2_auth_ping_requires_auth() {
    let server = common::TestServer::start().await;
    let client = server.client();

    let resp = client.get("/api2/auth/ping/", None).await;
    assert_eq!(resp.status(), 401);
}
