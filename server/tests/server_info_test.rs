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
    assert_eq!(
        body["encrypted_library_version"], 2,
        "seahub's ENCRYPTED_LIBRARY_VERSION default; clients take it verbatim \
         as the enc_version of a library they create"
    );

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
    for want in ["seafile-basic", "seafile-pro", "file-search"] {
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
        cfg.encrypted_library_pwd_hash_algo = Some("pbkdf2_sha256".to_string());
        cfg.encrypted_library_pwd_hash_params = Some("5000".to_string());
    })
    .await;
    let client = server.client();

    let resp = client.get("/api2/server-info/", None).await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["desktop-custom-brand"], "My Brand");
    assert_eq!(body["desktop-custom-logo"], "custom/logo.png");
    // Advertised verbatim: a client that sees the algorithm generates a
    // `pwd_hash` *instead of* a `magic` (seafile's
    // `seafile_generate_magic_and_random_key`), so the string has to be exactly
    // what seafile spells.
    assert_eq!(body["encrypted_library_pwd_hash_algo"], "pbkdf2_sha256");
    assert_eq!(body["encrypted_library_pwd_hash_params"], "5000");
}

/// seahub emits `encrypted_library_pwd_hash_params` whenever the algorithm is
/// configured — as `""` when there is no explicit parameter string — so a client
/// never sees the pair split.
#[tokio::test]
async fn test_server_info_pwd_hash_params_defaults_to_empty_string() {
    let server = common::TestServer::start_with_server_info_config(|cfg| {
        cfg.encrypted_library_pwd_hash_algo = Some("argon2id".to_string());
    })
    .await;
    let client = server.client();

    let resp = client.get("/api2/server-info/", None).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["encrypted_library_pwd_hash_algo"], "argon2id");
    assert_eq!(body["encrypted_library_pwd_hash_params"], "");
}

/// An algorithm the clients would compare against their own table must be
/// rejected at startup rather than advertised and then mis-honoured.
#[test]
fn test_encrypted_library_config_validation() {
    use infra::config::Config;

    let mut config = Config::default();

    config.server.encrypted_library_version = 3;
    assert!(
        config.server.validate_encrypted_library().is_err(),
        "enc_version 3 (AES-128-ECB) is not implemented"
    );
    config.server.encrypted_library_version = 2;
    assert!(config.server.validate_encrypted_library().is_ok());

    // Upstream's default: no algorithm means the legacy magic flow.
    config.server.encrypted_library_pwd_hash_algo = None;
    assert!(config.server.validate_encrypted_library().is_ok());
    config.server.encrypted_library_pwd_hash_algo = Some(String::new());
    assert!(config.server.validate_encrypted_library().is_ok());

    for algo in ["pbkdf2_sha256", "argon2id"] {
        config.server.encrypted_library_pwd_hash_algo = Some(algo.to_string());
        assert!(
            config.server.validate_encrypted_library().is_ok(),
            "{algo} must be accepted"
        );
    }

    // The clients compare this value verbatim, so a differently-cased or
    // differently-spelled name would make them fall back to their own table.
    for algo in ["PBKDF2", "pbkdf2", "scrypt", "argon2i"] {
        config.server.encrypted_library_pwd_hash_algo = Some(algo.to_string());
        assert!(
            config.server.validate_encrypted_library().is_err(),
            "{algo} must be rejected"
        );
    }

    // Parameters are bounded so a library can never be created with a
    // verification cost that later exhausts the server.
    config.server.encrypted_library_pwd_hash_algo = Some("argon2id".to_string());
    config.server.encrypted_library_pwd_hash_params = Some("2,102400,8".to_string());
    assert!(config.server.validate_encrypted_library().is_ok());
    config.server.encrypted_library_pwd_hash_params = Some("2,4294967295,8".to_string());
    assert!(config.server.validate_encrypted_library().is_err());
}

#[tokio::test]
async fn test_server_info_encrypted_library_version_is_configurable() {
    let server = common::TestServer::start_with_server_info_config(|cfg| {
        cfg.encrypted_library_version = 4;
    })
    .await;
    let client = server.client();

    let resp = client.get("/api2/server-info/", None).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["encrypted_library_version"], 4,
        "clients create libraries at exactly the advertised version"
    );
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
async fn test_server_info_file_search_gated_off() {
    let server = common::TestServer::start_with_server_info_config(|cfg| {
        cfg.file_search_enabled = false;
    })
    .await;
    let client = server.client();

    let resp = client.get("/api2/server-info/", None).await;
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    let features = body["features"].as_array().unwrap();
    assert!(
        !features.iter().any(|f| f == "file-search"),
        "file-search must not be advertised when disabled"
    );
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
