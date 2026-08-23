mod common;

use common::TestFixture;

/// B.4.1 — GET /api2/repos/{repo_id}/thumbnail/?p=&size=
#[tokio::test]
async fn test_thumbnail_not_found() {
    let f = TestFixture::new().await;

    // Upload a file (not an image) — thumbnail won't exist
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "test.txt", b"hello")
        .await;
    assert!(resp.status().is_success());

    // Request thumbnail for the text file — should 404 (can't generate thumbnail for text)
    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/thumbnail/?p=/test.txt&size=48", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn test_thumbnail_unauthorized() {
    let server = common::TestServer::start().await;
    let client = server.client();
    let resp = client
        .get("/api2/repos/some-repo/thumbnail/?p=/f.txt&size=48", None)
        .await;
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_thumbnail_directory_returns_400() {
    let f = TestFixture::new().await;

    let resp = f.client.create_dir(&f.api_token, &f.repo_id, "/pics").await;
    assert!(resp.status().is_success());

    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/thumbnail/?p=/pics&size=48", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 400);
}

/// Security: absurd thumbnail sizes must be rejected up-front, not passed to
/// the image decoder (which would allocate size×size×bytes and OOM the process).
#[tokio::test]
async fn test_thumbnail_oversized_size_rejected() {
    let f = TestFixture::new().await;

    let resp = f
        .client
        .get(
            &format!(
                "/api2/repos/{}/thumbnail/?p=/test.txt&size=100000",
                f.repo_id
            ),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 400);
}

/// TIFF (the newly added in-process image format) thumbnails are served as PNG.
#[tokio::test]
async fn test_tiff_thumbnail_generated() {
    let f = TestFixture::new().await;

    let mut img = image::RgbaImage::new(32, 32);
    for p in img.pixels_mut() {
        *p = image::Rgba([1u8, 2, 3, 255]);
    }
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Tiff)
        .expect("TIFF encode failed");

    let resp = f
        .client
        .upload_file(
            &f.api_token,
            &f.repo_id,
            "/",
            "photo.tiff",
            &buf.into_inner(),
        )
        .await;
    assert!(resp.status().is_success());

    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/thumbnail/?p=/photo.tiff&size=48", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok());
    assert_eq!(ct, Some("image/png"));
}

/// Thumbnails carry a strong ETag (SHA-1 of the PNG bytes); a matching
/// `If-None-Match` returns 304, and editing the file changes the validator.
#[tokio::test]
async fn test_thumbnail_etag_conditional_request() {
    let f = TestFixture::new().await;

    fn make_tiff(pixel: [u8; 3]) -> Vec<u8> {
        let mut img = image::RgbaImage::new(32, 32);
        for p in img.pixels_mut() {
            *p = image::Rgba([pixel[0], pixel[1], pixel[2], 255]);
        }
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Tiff)
            .expect("TIFF encode failed");
        buf.into_inner()
    }

    let resp = f
        .client
        .upload_file(
            &f.api_token,
            &f.repo_id,
            "/",
            "photo.tiff",
            &make_tiff([1, 2, 3]),
        )
        .await;
    assert!(resp.status().is_success());

    let url = format!(
        "{}/api2/repos/{}/thumbnail/?p=/photo.tiff&size=48",
        f.server.base_url, f.repo_id
    );
    let client = reqwest::Client::new();

    // 200 + ETag + body.
    let resp = client
        .get(&url)
        .bearer_auth(&f.api_token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let etag = resp
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .to_string();
    assert!(!resp.bytes().await.unwrap().is_empty());

    // Matching validator → 304 without a body.
    let resp = client
        .get(&url)
        .bearer_auth(&f.api_token)
        .header("if-none-match", &etag)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 304);

    // Editing the file (replace upload) changes the thumbnail → new ETag. The
    // 1.1s pause ensures a new source mtime so the staleness check regenerates
    // (mtime is second-resolution).
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let resp = f
        .client
        .upload_file(
            &f.api_token,
            &f.repo_id,
            "/",
            "photo.tiff",
            &make_tiff([9, 9, 9]),
        )
        .await;
    assert!(resp.status().is_success());
    let resp = client
        .get(&url)
        .bearer_auth(&f.api_token)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let new_etag = resp
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .to_string();
    assert_ne!(etag, new_etag, "edited file must change the thumbnail ETag");
}
