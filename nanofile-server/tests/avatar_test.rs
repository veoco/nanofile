mod common;

use common::TestFixture;

/// B.5.1 — GET /api2/avatars/user/{email}/resized/{size}/
#[tokio::test]
async fn test_avatar_default() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .get(
            &format!("/api2/avatars/user/{}/resized/80/", f.email),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["is_default"].as_bool().unwrap_or(false));
    assert!(body["url"].as_str().unwrap_or("").contains("avatars"));
}

#[tokio::test]
async fn test_avatar_different_sizes() {
    let f = TestFixture::new().await;

    for size in &["32", "48", "80", "128"] {
        let resp = f
            .client
            .get(
                &format!("/api2/avatars/user/{}/resized/{}/", f.email, size),
                Some(&f.api_token),
            )
            .await;
        assert_eq!(resp.status(), 200, "size={} failed", size);
    }
}

#[tokio::test]
async fn test_avatar_nonexistent_user() {
    let f = TestFixture::new().await;

    // Seahub compatibility: nonexistent users get a default avatar URL, not 404.
    let resp = f
        .client
        .get(
            "/api2/avatars/user/nobody@test.com/resized/80/",
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["is_default"].as_bool().unwrap_or(false));
    assert!(body["url"].as_str().unwrap_or("").contains("avatars"));
}
