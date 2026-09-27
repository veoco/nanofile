//! What each sandbox-backed feature does when the image worker cannot run.
//!
//! Its own integration-test binary because the worker's executable is
//! process-global and set once: every other test's fixture points it at the
//! real binary and then insists the sandbox confines it. This file points it at
//! something that does not exist *before* the first fixture, which is the only
//! way to reach the unavailable path from the outside — the same shape a host
//! below `sandbox.min_level` has from the parent's side.
//!
//! The behaviour under test is fail-closed. Three paths decode bytes a user
//! chose — the avatar upload, the EXIF endpoint and the file thumbnail — and
//! with the image profile unavailable each has to refuse rather than decode
//! them in this process. A 200 with no thumbnail, or `null` from an EXIF read
//! the worker never ran, would be the failure this file exists to catch.

mod common;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use common::TestFixture;

/// Point the worker at a path that cannot be started, once, before any fixture
/// is built.
///
/// `OnceLock` rather than a plain call because the tests below run on their own
/// threads: whoever gets there first configures, the rest wait, and no fixture
/// can be built before the path is set. Every test in this file offers the same
/// path, so whichever thread wins the set-once race the answer is the same.
fn the_image_worker_cannot_run() {
    static CONFIGURED: OnceLock<()> = OnceLock::new();
    CONFIGURED.get_or_init(|| {
        assert!(
            server::sandbox::worker::configure_executable(PathBuf::from(
                "/nonexistent/nanofile-extract-worker"
            )),
            "this file must be the first to configure the worker"
        );
    });
}

/// A one-pixel PNG: small enough that no size limit is what refuses it.
fn a_png() -> Vec<u8> {
    let mut image = image::RgbImage::new(1, 1);
    image.put_pixel(0, 0, image::Rgb([10, 20, 30]));
    let mut bytes = Vec::new();
    image
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .expect("encode a PNG");
    bytes
}

/// Every regular file under `directory`, at any depth.
fn stored_files(directory: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(next) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else {
                found.push(path);
            }
        }
    }
    found
}

/// An avatar is refused, and nothing of it is kept.
///
/// The refusal has to land *before* the bytes are stored: a host that cannot
/// confine the decoder must not hold the upload either, or the profile being
/// unavailable would only move the decode to a later request.
#[tokio::test]
async fn an_avatar_upload_is_refused_without_the_image_worker() {
    the_image_worker_cannot_run();
    let f = TestFixture::new().await;

    let form = reqwest::multipart::Form::new().part(
        "avatar",
        reqwest::multipart::Part::bytes(a_png()).file_name("a.png"),
    );
    let resp = f
        .client
        .post_multipart("/api/v2.1/user-avatar/", Some(&f.api_token), form)
        .await;
    assert_eq!(
        resp.status(),
        400,
        "an upload nothing can decode must be refused"
    );
    let body: serde_json::Value = resp.json().await.expect("the refusal is JSON");
    let reason = body["error_msg"].as_str().unwrap_or_default();
    assert!(
        reason.contains("avatar processing is unavailable"),
        "the refusal must name the environment it needs: {body}"
    );

    let avatar_dir = f.server.state.config().storage.avatar_dir.clone();
    assert!(
        stored_files(&avatar_dir).is_empty(),
        "a refused avatar must leave no file behind: {:?}",
        stored_files(&avatar_dir)
    );
}

/// EXIF is an error, not a silent `null`.
///
/// `null` is what a file with no EXIF returns, so answering it for a worker
/// that never ran would be indistinguishable from a valid answer — and the
/// caller would have no way to tell that this host grew no EXIF support.
#[tokio::test]
async fn exif_is_an_error_without_the_image_worker() {
    the_image_worker_cannot_run();
    let f = TestFixture::new().await;

    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "shot.png", &a_png())
        .await;
    assert_eq!(resp.status(), 200, "upload should succeed");

    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/file/exif/?p=/shot.png", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(
        resp.status(),
        500,
        "an unmeasured EXIF read is an environment error, not `null`"
    );

    // The reason itself: `AppError::Internal` is answered as a generic 500, so
    // the service is where the sentence the operator sees in the log is
    // asserted.
    let error = f
        .server
        .state
        .exif_service()
        .get_exif(&f.repo_id, "/shot.png")
        .await
        .expect_err("an unavailable worker is not a successful EXIF read");
    let reason = error.to_string();
    assert!(
        reason.contains("EXIF processing is unavailable"),
        "the error must name the environment it needs: {reason}"
    );
}

/// A thumbnail is not produced, and none is cached.
#[tokio::test]
async fn a_thumbnail_is_not_generated_without_the_image_worker() {
    the_image_worker_cannot_run();
    let f = TestFixture::new().await;

    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "shot.png", &a_png())
        .await;
    assert_eq!(resp.status(), 200, "upload should succeed");

    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/thumbnail/?p=/shot.png&size=48", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(
        resp.status(),
        404,
        "a thumbnail no worker could decode must not be served"
    );

    let thumbnail_dir = f.server.state.config().storage.thumbnail_dir.clone();
    assert!(
        stored_files(&thumbnail_dir).is_empty(),
        "a refused thumbnail must not be cached: {:?}",
        stored_files(&thumbnail_dir)
    );
}
