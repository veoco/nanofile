//! Media thumbnails are produced by the sandbox worker, not in this process.
//!
//! The image path is covered where thumbnails are tested generally; this file
//! covers the one pipeline that executes an external program, because that is
//! the pipeline whose confinement can fail in a way a thumbnail test would see
//! as "no thumbnail" rather than as a broken sandbox.

mod common;

use common::TestFixture;

/// Generate a one-second clip, or answer false when the host has no ffmpeg.
fn generate_clip(path: &std::path::Path) -> bool {
    std::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=1:size=320x240:rate=10",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(path)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// The media profile confines itself on this host before any request is served.
#[tokio::test]
async fn the_media_profile_confines_itself() {
    if !generate_clip(&std::env::temp_dir().join("nanofile-nonexistent-probe.mp4")) {
        eprintln!("ffmpeg not available; skipping media sandbox test");
        return;
    }
    let _f = TestFixture::new().await;
    match server::sandbox::worker::status(server::sandbox::Profile::Media) {
        server::sandbox::worker::Status::Ready(report) => {
            assert!(
                report.level() >= server::sandbox::Level::Partial,
                "the media worker must be confined: {}",
                report.detail
            );
            assert!(
                report.detail.contains("parse=media-ok"),
                "the confined worker must still run the helper: {}",
                report.detail
            );
        }
        server::sandbox::worker::Status::Unavailable(why) => {
            panic!("the media sandbox is unavailable: {why}")
        }
    }
}

/// A video uploaded to a library gets a thumbnail, and that thumbnail came from
/// the confined worker: ffmpeg ran under a profile that may execute exactly that
/// one helper and read exactly the scratch file it was handed.
#[tokio::test]
async fn a_video_thumbnail_is_generated_in_the_sandbox() {
    let dir = tempfile::tempdir().expect("temp dir");
    let clip = dir.path().join("clip.mp4");
    if !generate_clip(&clip) {
        eprintln!("ffmpeg not available; skipping media thumbnail test");
        return;
    }
    let bytes = std::fs::read(&clip).expect("read the clip");

    let f = TestFixture::new().await;
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "clip.mp4", &bytes)
        .await;
    assert!(resp.status().is_success(), "upload should succeed");

    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/thumbnail/?p=/clip.mp4&size=48", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(
        resp.status(),
        200,
        "a video the confined worker can read must get a thumbnail"
    );
    let data = resp.bytes().await.expect("bytes");
    let decoded = image::load_from_memory(&data).expect("the reply is an image");
    assert!(decoded.width() > 0 && decoded.height() > 0);
}
