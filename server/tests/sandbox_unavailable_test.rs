//! What the server does when the extraction worker cannot run.
//!
//! Points the worker at a path that cannot be started *before* the first
//! fixture, which is the only way to reach the unavailable path from outside.
//! The executable is process-global, so this is a binary of its own, and nothing
//! here may build a fixture before it is pointed away.

mod common;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use common::TestFixture;
use server::indexer::DocStatus;

/// Point the worker at a path that cannot be started, once, before any fixture.
///
/// `OnceLock` because the tests run on their own threads: whoever gets there
/// first configures and the rest wait, so no fixture can be built first.
fn the_worker_cannot_run() {
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

/// A document the worker could not carry is a retryable failure, is not parsed
/// in this process, and does not stop what needs no worker.
#[tokio::test]
async fn a_document_is_not_indexed_without_the_worker() {
    the_worker_cannot_run();

    let f = common::TestFixture::new_with_index().await;
    let token = &f.api_token;
    let path = "/report.pdf";

    let resp = f
        .client
        .upload_file(
            token,
            &f.repo_id,
            "/",
            "report.pdf",
            &common::minimal_pdf("zebraquartz"),
        )
        .await;
    assert_eq!(resp.status(), 200, "upload should succeed");

    let resp = f
        .client
        .post_json(
            &format!("/api2/repos/{}/file/reindex/", f.repo_id),
            Some(token),
            &serde_json::json!({ "p": path }),
        )
        .await;
    assert_eq!(resp.status(), 200, "the request itself must succeed");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["indexed"].as_bool(),
        Some(false),
        "a document the worker could not carry is not indexed"
    );

    // The server has to be alive and still able to index what needs no worker.
    let resp = f
        .client
        .upload_file(token, &f.repo_id, "/", "after.txt", b"aftermath signal")
        .await;
    assert_eq!(resp.status(), 200);
    assert!(
        wait_for(Duration::from_secs(30), || async {
            !search_results(&f, token, "aftermath").await.is_empty()
        })
        .await,
        "text needs no worker, so it must still be indexed"
    );

    // Failed, not skipped: the environment is the problem, so the retry that a
    // fixed host needs has to stay open.
    let indexer = f.server.state.indexer.clone().expect("indexer is enabled");
    let states = indexer
        .doc_states(&f.repo_id, &[path.to_string()])
        .expect("read document states");
    assert_eq!(
        states.get(path).map(|state| state.status.clone()),
        Some(DocStatus::Failed),
        "an unavailable worker is a retryable failure, not a verdict on the file"
    );
}

/// An avatar is refused, and nothing of it is kept.
///
/// The refusal has to land *before* the bytes are stored: a host that cannot
/// confine the decoder must not hold the upload either, or the profile being
/// unavailable would only move the decode to a later request.
#[tokio::test]
async fn an_avatar_upload_is_refused_without_the_image_worker() {
    the_worker_cannot_run();
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
    the_worker_cannot_run();
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
    the_worker_cannot_run();
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

/// Perform a full-text content search and return the `results` array.
async fn search_results(f: &common::TestFixture, token: &str, q: &str) -> Vec<serde_json::Value> {
    let resp = f
        .client
        .get(
            &format!("/api2/search/?q={q}&search_filename_only=false"),
            Some(token),
        )
        .await;
    assert_eq!(resp.status(), 200);
    resp.json::<serde_json::Value>().await.unwrap()["results"]
        .as_array()
        .unwrap()
        .clone()
}

async fn wait_for<F, Fut>(timeout: Duration, mut predicate: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}
