use crate::common;

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use common::TestFixture;

/// A small opaque PNG.
fn png_bytes(width: u32, height: u32, pixel: [u8; 3]) -> Vec<u8> {
    let mut img = image::RgbImage::new(width, height);
    for p in img.pixels_mut() {
        *p = image::Rgb(pixel);
    }
    let mut bytes = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut bytes),
        image::ImageFormat::Png,
    )
    .expect("PNG encode failed");
    bytes
}

async fn upload_avatar(f: &TestFixture, name: &str, bytes: Vec<u8>) -> reqwest::Response {
    let form = reqwest::multipart::Form::new().part(
        "avatar",
        reqwest::multipart::Part::bytes(bytes).file_name(name.to_string()),
    );
    f.client
        .post_multipart("/api/v2.1/user-avatar/", Some(&f.api_token), form)
        .await
}

async fn get_avatar_image(f: &TestFixture, size: u32) -> reqwest::Response {
    f.client
        .get(&format!("/avatar/render_primary/{}/{size}/", f.email), None)
        .await
}

/// The one avatar storage directory of the fixture's only user.
fn avatar_storage_dir(avatar_dir: &Path) -> PathBuf {
    let mut entries = std::fs::read_dir(avatar_dir)
        .expect("the avatar directory exists")
        .flatten();
    let entry = entries.next().expect("one avatar storage directory");
    assert!(entries.next().is_none(), "one user, one avatar directory");
    entry.path()
}

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
    assert!(
        body["url"]
            .as_str()
            .unwrap_or("")
            .contains("/avatar/render_primary/")
    );
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
        .get(&format!("/avatar/render_primary/{}/100000/", f.email), None)
        .await;
    assert_eq!(resp.status(), 400);
}

/// The image route's pre-move root spelling answers a permanent redirect into the
/// `/api/` namespace: avatar URLs are embedded in responses and pages, so a
/// browser holding one must keep rendering the image.
#[tokio::test]
async fn test_avatar_image_legacy_path_redirects() {
    let f = TestFixture::new().await;

    // The shared test client follows redirects; use the no-redirect flavour so
    // the permanent redirect and its `Location` stay visible.
    let ui = common::client::TestClient::new_ui_session(&f.server.base_url);
    let resp = ui
        .get(&format!("/avatars/user/{}/resized/80/", f.email), None)
        .await;
    assert_eq!(resp.status(), 308, "the old image path must redirect");
    let expected = format!("/avatar/render_primary/{}/80/", f.email);
    assert_eq!(
        resp.headers()
            .get("location")
            .and_then(|value| value.to_str().ok()),
        Some(expected.as_str()),
        "the redirect must land on the render_primary avatar route"
    );
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
    assert!(
        body["url"]
            .as_str()
            .unwrap_or("")
            .contains("/avatar/render_primary/")
    );
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
    let resp = common::client::response_or_early_close(
        f.client
            .try_post_multipart("/api/v2.1/user-avatar/", Some(&f.api_token), form)
            .await,
    );
    // The part is refused as soon as the cap is passed, so the connection may
    // close while the rest of it is still going out; `None` is that early close.
    if let Some(resp) = resp {
        assert_eq!(
            resp.status(),
            413,
            "oversized avatar must be rejected with 413"
        );
    }

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

    // The thumbnail is the confined worker's, produced behind the image gate and
    // off the runtime: a 200 with no file on disk would mean that path never ran.
    let avatar_dir = f.server.state.config().storage.avatar_dir.clone();
    let wrote_256 = std::fs::read_dir(&avatar_dir)
        .expect("the avatar directory exists")
        .flatten()
        .any(|entry| entry.path().join("256.png").exists());
    assert!(wrote_256, "the sandbox worker wrote no 256 avatar");
}

/// The image route is unauthenticated, so a resize the worker cannot produce is
/// remembered for an hour in a `{size}.failed` marker instead of starting a confined
/// child on every request. A fresh marker serves the original; once it expires the
/// next request tries again, and success clears it.
#[tokio::test]
async fn test_avatar_failed_thumbnail_is_negative_cached() {
    let f = TestFixture::new().await;

    // A valid avatar: the upload materialises the 256 entry, so 48 is generated on
    // demand.
    let resp = upload_avatar(&f, "me.png", png_bytes(64, 32, [10, 20, 30])).await;
    assert_eq!(resp.status(), 200, "avatar upload should succeed");

    let avatar_dir = f.server.state.config().storage.avatar_dir.clone();
    let storage = avatar_storage_dir(&avatar_dir);
    let thumbnail = storage.join("48.png");
    let marker = storage.join("48.failed");
    assert!(storage.join("256.png").exists(), "the upload writes 256");
    assert!(!thumbnail.exists(), "the 48 entry is generated on demand");

    // A marker written now stands in for a generation the worker failed earlier.
    std::fs::write(&marker, b"failed\n").expect("write the marker");
    let resp = get_avatar_image(&f, 48).await;
    assert_eq!(resp.status(), 200);
    let body = resp.bytes().await.expect("bytes");
    let decoded = image::load_from_memory(&body).expect("the original is an image");
    assert_eq!(
        (decoded.width(), decoded.height()),
        (64, 32),
        "the original is served while the marker holds"
    );
    assert!(
        !thumbnail.exists(),
        "a fresh marker must answer without starting a generation"
    );

    // An expired marker lets the request try again: generation runs, succeeds, and
    // the marker and a fresh 48 entry are written instead.
    let stale = SystemTime::now() - Duration::from_secs(2 * 60 * 60);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&marker)
        .expect("open the marker")
        .set_modified(stale)
        .expect("backdate the marker");
    let resp = get_avatar_image(&f, 48).await;
    assert_eq!(resp.status(), 200);
    let body = resp.bytes().await.expect("bytes");
    let decoded = image::load_from_memory(&body).expect("the thumbnail is an image");
    assert_eq!(
        (decoded.width(), decoded.height()),
        (48, 48),
        "an expired marker must allow the resize again"
    );
    assert!(thumbnail.exists(), "the successful resize is cached");
    assert!(!marker.exists(), "success clears the failure marker");
}

/// A resize that fails is remembered, and the original keeps being served: the
/// route answers with the uploaded bytes rather than a default or an error.
#[tokio::test]
async fn test_avatar_failed_generation_writes_the_marker_and_serves_the_original() {
    let f = TestFixture::new().await;

    // The upload accepts the file (extension and size only); the decode fails.
    let corrupt = b"this is not a PNG".to_vec();
    let resp = upload_avatar(&f, "broken.png", corrupt.clone()).await;
    assert_eq!(
        resp.status(),
        200,
        "the upload only fails on the resize, which is non-fatal"
    );

    let avatar_dir = f.server.state.config().storage.avatar_dir.clone();
    let storage = avatar_storage_dir(&avatar_dir);
    let marker = storage.join("48.failed");
    assert!(
        !marker.exists(),
        "the upload records nothing for a size it never generated"
    );

    let resp = get_avatar_image(&f, 48).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("image/png"),
        "the original's own type, resolved from its extension"
    );
    let body = resp.bytes().await.expect("bytes");
    assert_eq!(
        body.as_ref(),
        corrupt.as_slice(),
        "the uploaded bytes are what a failed resize serves"
    );
    let age = std::fs::metadata(&marker)
        .expect("the failure marker was written")
        .modified()
        .expect("the marker carries an mtime")
        .elapsed()
        .expect("the marker is in the past");
    assert!(
        age < Duration::from_secs(60),
        "the failure marker must be fresh, not {age:?} old"
    );
    assert!(!storage.join("48.png").exists(), "no thumbnail was written");

    // The next request is answered from the marker: same original, no new marker
    // needed, and still no thumbnail.
    let resp = get_avatar_image(&f, 48).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.bytes().await.expect("bytes").as_ref(),
        corrupt.as_slice()
    );
    assert!(!storage.join("48.png").exists());
}

/// A new upload throws away everything derived from the previous avatar — the
/// materialised sizes and any failure marker — before the new original is written,
/// so none of it can be served for the new picture.
#[tokio::test]
async fn test_avatar_upload_replaces_generated_files() {
    let f = TestFixture::new().await;

    let resp = upload_avatar(&f, "old.png", png_bytes(32, 32, [1, 2, 3])).await;
    assert_eq!(resp.status(), 200);

    let avatar_dir = f.server.state.config().storage.avatar_dir.clone();
    let storage = avatar_storage_dir(&avatar_dir);
    let stale_size = storage.join("48.png");
    let stale_marker = storage.join("48.failed");
    std::fs::write(&stale_size, b"stale size").expect("write a stale size");
    std::fs::write(&stale_marker, b"failed\n").expect("write a stale marker");

    let new_bytes = png_bytes(8, 8, [9, 9, 9]);
    let resp = upload_avatar(&f, "new.png", new_bytes.clone()).await;
    assert_eq!(resp.status(), 200);

    assert!(!stale_size.exists(), "a generated size outlived the upload");
    assert!(
        !stale_marker.exists(),
        "a failure marker outlived the upload"
    );
    let original = std::fs::read(storage.join("original.png")).expect("the new original");
    assert_eq!(
        original, new_bytes,
        "the new original is the one that was uploaded"
    );
    assert!(
        storage.join("256.png").exists(),
        "the new avatar's 256 entry is generated after the cleanup"
    );
}
