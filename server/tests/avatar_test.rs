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

/// Security: the unauthenticated avatar-image endpoint must reject oversized
/// sizes (which would otherwise trigger a huge thumbnail allocation / OOM).
#[tokio::test]
async fn test_avatar_image_oversized_size_rejected() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .get(&format!("/avatars/user/{}/resized/100000/", f.email), None)
        .await;
    assert_eq!(resp.status(), 400);
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

/// Security: avatar uploads must cap the multipart part at 1 MiB before
/// buffering, so an authenticated user can't force a multi-GB allocation.
#[tokio::test]
async fn test_avatar_upload_size_limited() {
    let f = TestFixture::new().await;

    // Oversized part (2 MiB) → 413, far under the 4 GiB global upload limit.
    let form = reqwest::multipart::Form::new().part(
        "avatar",
        reqwest::multipart::Part::bytes(vec![0u8; 2 * 1024 * 1024]).file_name("a.png"),
    );
    let resp = f
        .client
        .post_multipart("/api/v2.1/user-avatar/", Some(&f.api_token), form)
        .await;
    assert_eq!(
        resp.status(),
        413,
        "oversized avatar must be rejected with 413"
    );

    // A real small PNG still uploads fine.
    let png = {
        let mut img = image::RgbImage::new(1, 1);
        img.put_pixel(0, 0, image::Rgb([10, 20, 30]));
        let mut bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        bytes
    };
    let form = reqwest::multipart::Form::new().part(
        "avatar",
        reqwest::multipart::Part::bytes(png).file_name("a.png"),
    );
    let resp = f
        .client
        .post_multipart("/api/v2.1/user-avatar/", Some(&f.api_token), form)
        .await;
    assert_eq!(resp.status(), 200, "small PNG avatar should upload");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["avatar_url"].as_str().is_some(), "avatar_url missing");
}
