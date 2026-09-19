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

/// TIFF (the newly added in-process image format) thumbnails are served as
/// JPEG: the pixels are opaque, and JPEG costs ~7x fewer bytes than PNG for
/// photographic content.
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
    assert_eq!(ct, Some("image/jpeg"));
    // The bytes really are a JPEG (SOI marker), not a PNG with a JPEG label.
    let body = resp.bytes().await.unwrap();
    assert_eq!(&body[..2], &[0xFF, 0xD8], "served body is not a JPEG");
}

/// Encode an image as PNG bytes for upload.
fn png_bytes(img: &image::DynamicImage) -> Vec<u8> {
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png)
        .expect("PNG encode failed");
    buf.into_inner()
}

/// The container follows the pixels, and the choice is part of the cache
/// entry: an opaque image is served as JPEG with a `.jpg` cache file, while an
/// image that actually uses transparency stays PNG. Both repeat requests are
/// served from the cache with the same validator, so the format must have been
/// persisted rather than re-derived by decoding again.
#[tokio::test]
async fn test_thumbnail_container_follows_pixels_and_is_cached() {
    let f = TestFixture::new().await;

    // Opaque pixels in an RGB PNG → JPEG. Fully transparent RGBA → PNG.
    let opaque = png_bytes(&image::DynamicImage::ImageRgb8(
        image::RgbImage::from_pixel(32, 32, image::Rgb([10, 120, 200])),
    ));
    let transparent = png_bytes(&image::DynamicImage::ImageRgba8(image::RgbaImage::new(
        32, 32,
    )));

    let cache_dir = f
        .server
        .block_dir
        .parent()
        .expect("block dir has a parent")
        .join("thumbnails")
        .join(infra::crypto::fs_id::sha1_hex(f.repo_id.as_bytes()));

    let read_names = || -> Vec<String> {
        // The repo's cache directory is created lazily by the first miss.
        std::fs::read_dir(&cache_dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect()
    };

    for (name, bytes, want_mime, want_db, want_ext, other_ext) in [
        ("opaque.png", &opaque, "image/jpeg", "jpeg", "jpg", "png"),
        ("alpha.png", &transparent, "image/png", "png", "png", "jpg"),
    ] {
        let before = read_names();
        let resp = f
            .client
            .upload_file(&f.api_token, &f.repo_id, "/", name, bytes)
            .await;
        assert!(resp.status().is_success());

        let url = format!("/api2/repos/{}/thumbnail/?p=/{name}&size=48", f.repo_id);
        let etag = {
            let resp = f.client.get(&url, Some(&f.api_token)).await;
            assert_eq!(resp.status(), 200, "{name}");
            assert_eq!(
                resp.headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok()),
                Some(want_mime),
                "{name}"
            );
            let etag = resp
                .headers()
                .get("etag")
                .and_then(|v| v.to_str().ok())
                .unwrap()
                .to_string();
            let body = resp.bytes().await.unwrap();
            let magic: &[u8] = if want_mime == "image/jpeg" {
                &[0xFF, 0xD8]
            } else {
                b"\x89PNG"
            };
            assert_eq!(&body[..magic.len()], magic, "{name} body magic");
            etag
        };

        // Second request: served from the cache (the format came from the row,
        // not from re-decoding), same validator, same type.
        let resp = f.client.get(&url, Some(&f.api_token)).await;
        assert_eq!(resp.status(), 200, "{name} second request");
        assert_eq!(
            resp.headers().get("etag").and_then(|v| v.to_str().ok()),
            Some(etag.as_str()),
            "{name} second request must reuse the cached thumbnail"
        );
        assert_eq!(
            resp.headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok()),
            Some(want_mime),
            "{name} second request"
        );

        // The recorded container and the on-disk entry agree with the response.
        let record = f
            .server
            .repos
            .thumbnail
            .find_by_repo_path_size(&f.repo_id, &format!("/{name}"), 48)
            .await
            .unwrap()
            .expect("thumbnail cache row");
        assert_eq!(record.format, want_db, "{name} stored format");

        let names: Vec<String> = read_names();
        // Exactly one cache entry appeared, and it is named for the container
        // that was actually written — a stale entry in the other container
        // would show up here as a second file sharing this file's hash.
        let added: Vec<&String> = names.iter().filter(|n| !before.contains(n)).collect();
        assert_eq!(
            added.len(),
            1,
            "{name}: expected exactly one new cache file, got {added:?}"
        );
        assert!(
            added[0].ends_with(&format!("_48.{want_ext}")),
            "{name}: expected a _48.{want_ext} cache file, got {}",
            added[0]
        );
        let hash_prefix = added[0]
            .rsplit_once('_')
            .expect("cache file name contains the size separator")
            .0;
        let same_file: Vec<&String> = names
            .iter()
            .filter(|n| n.starts_with(hash_prefix))
            .collect();
        assert_eq!(
            same_file.len(),
            1,
            "{name}: a stale _48.{other_ext} entry was left behind: {same_file:?}"
        );
    }
}

/// Thumbnails carry a strong ETag (SHA-1 of the encoded bytes); a matching
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
