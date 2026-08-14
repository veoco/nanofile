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
    // The local-browser SSO flow is implemented and enabled by default, so the
    // feature must be advertised — desktop/mobile clients use it to show the
    // SSO login entry.
    assert!(
        features.iter().any(|f| f == "client-sso-via-local-browser"),
        "client-sso-via-local-browser should be advertised when sso_enabled"
    );
}

#[tokio::test]
async fn test_server_info_sso_feature_gated_off() {
    let server = common::TestServer::start_with_sso_enabled(false).await;
    let client = server.client();

    let resp = client.get("/api2/server-info/", None).await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let features = body["features"].as_array().unwrap();
    assert!(
        !features.iter().any(|f| f == "client-sso-via-local-browser"),
        "client-sso-via-local-browser must not be advertised when sso_enabled=false"
    );
}

#[tokio::test]
async fn test_server_info_optional_fields_advertised_when_configured() {
    let server = common::TestServer::start_with_server_info_config(|cfg| {
        cfg.desktop_custom_brand = Some("My Brand".to_string());
        cfg.desktop_custom_logo = Some("custom/logo.png".to_string());
        cfg.encrypted_library_pwd_hash_algo = Some("PBKDF2".to_string());
        cfg.encrypted_library_pwd_hash_params = Some("iterations=1000".to_string());
    })
    .await;
    let client = server.client();

    let resp = client.get("/api2/server-info/", None).await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["desktop-custom-brand"], "My Brand");
    assert_eq!(body["desktop-custom-logo"], "custom/logo.png");
    assert_eq!(body["encrypted_library_pwd_hash_algo"], "PBKDF2");
    assert_eq!(body["encrypted_library_pwd_hash_params"], "iterations=1000");
}

#[tokio::test]
async fn test_server_info_optional_fields_absent_by_default() {
    let server = common::TestServer::start().await;
    let client = server.client();

    let resp = client.get("/api2/server-info/", None).await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    for key in [
        "desktop-custom-brand",
        "desktop-custom-logo",
        "encrypted_library_pwd_hash_algo",
        "encrypted_library_pwd_hash_params",
    ] {
        assert!(
            body.get(key).is_none(),
            "{key} should be absent unless configured"
        );
    }
}

#[tokio::test]
async fn test_server_info_file_search_and_wiki_gated_off() {
    let server = common::TestServer::start_with_server_info_config(|cfg| {
        cfg.file_search_enabled = false;
        cfg.wiki_enabled = false;
    })
    .await;
    let client = server.client();

    let resp = client.get("/api2/server-info/", None).await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let features = body["features"].as_array().unwrap();
    for not_want in ["file-search", "wiki"] {
        assert!(
            !features.iter().any(|f| f == not_want),
            "{not_want} must not be advertised when disabled"
        );
    }
    // The base features must remain even with the switches off.
    for want in ["seafile-basic", "seafile-pro"] {
        assert!(features.iter().any(|f| f == want), "{want} feature missing");
    }
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

    // Matches seahub's AuthPing, which answers `Response('pong')`.
    let body: String = resp.text().await.unwrap();
    assert_eq!(body, "\"pong\"");
}

#[tokio::test]
async fn test_ping_at_api2_auth_ping_requires_auth() {
    let server = common::TestServer::start().await;
    let client = server.client();

    let resp = client.get("/api2/auth/ping/", None).await;
    assert_eq!(resp.status(), 401);
}
