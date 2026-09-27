//! Media thumbnails are produced by the sandbox worker, not in this process.
//!
//! The image path is covered where thumbnails are tested generally; this file
//! covers the one pipeline that executes an external program, because that is
//! the pipeline whose confinement can fail in a way a thumbnail test would see
//! as "no thumbnail" rather than as a broken sandbox. It runs on every platform
//! that installs the helper, which is what makes the file authorization, the
//! decode and the resize measured rather than described: a host with no helper
//! fails here rather than passing with the pipeline untested.

mod common;

use std::sync::OnceLock;

use common::TestFixture;

/// The helper the sandbox is granted, and the one that writes the clips.
///
/// `NANOFILE_TEST_FFMPEG_PATH` names the binary CI installed. The server would
/// otherwise resolve `ffmpeg` on its own `PATH`, which on Windows and macOS is
/// often a package manager's shim: the media grant reaches the one file the
/// parent names and nothing it starts, so a shim would be measured as its own
/// refused second generation.
fn helper() -> &'static str {
    static HELPER: OnceLock<String> = OnceLock::new();
    HELPER.get_or_init(|| {
        std::env::var("NANOFILE_TEST_FFMPEG_PATH").unwrap_or_else(|_| "ffmpeg".to_string())
    })
}

/// Skip the test on a development host, fail it on CI.
///
/// A host without the helper can do nothing here, but a green suite that never
/// ran the media pipeline is the failure this file exists to catch — and that is
/// what a silent `return` produced before.
fn without_ffmpeg() {
    if std::env::var_os("CI").is_some() {
        panic!(
            "{} is not runnable; CI installs ffmpeg on this platform",
            helper()
        );
    }
    eprintln!(
        "{} is not available; skipping the media sandbox test",
        helper()
    );
}

/// Whether the helper runs at all.
fn helper_runs() -> bool {
    std::process::Command::new(helper())
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Generate a one-second clip with the helper, or answer false when the host
/// has none.
fn generate_clip(path: &std::path::Path) -> bool {
    std::process::Command::new(helper())
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

/// The media profile confines itself, decodes under the confinement, and starts
/// the helper it is granted.
#[tokio::test]
async fn the_media_profile_confines_itself() {
    if !helper_runs() {
        without_ffmpeg();
        return;
    }
    let _f = TestFixture::new_with_media_helper(helper()).await;
    match server::sandbox::worker::status(server::sandbox::Profile::Media) {
        server::sandbox::worker::Status::Ready(report) => {
            assert!(
                report.level() >= server::sandbox::Level::Partial,
                "the media worker must be confined: {}",
                report.detail
            );
            assert!(
                report.detail.contains("parse=media-ok("),
                "the confined worker must decode a frame, not merely start: {}",
                report.detail
            );
            // The process item is the one this profile cannot have: it starts a
            // program by definition. The report says so instead of claiming a
            // bound the profile does not have, and the helper it *may* start is
            // a fact beside it.
            assert!(
                !report.protections.process,
                "the media profile must not claim the process item: {}",
                report.detail
            );
            assert!(
                report.detail.contains("helper=allowed"),
                "the helper grant must be reported: {}",
                report.detail
            );
            // The fork the media profile needs is a unix measurement; Windows
            // has no fork, and says what it does bound instead: the Job Object's
            // process slots, one of which the helper takes.
            #[cfg(unix)]
            assert!(
                report.detail.contains("fork=open"),
                "the fork the media profile needs must be measured and said: {}",
                report.detail
            );
            #[cfg(windows)]
            assert!(
                report.detail.contains("processes2"),
                "the media profile's second process slot must be said: {}",
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
        without_ffmpeg();
        return;
    }
    let bytes = std::fs::read(&clip).expect("read the clip");

    let f = TestFixture::new_with_media_helper(helper()).await;
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
    // The frame the confined worker produced was 320x240 and the same-ratio fit
    // is exact, so the geometry says a real decode and resize happened rather
    // than an image of some other size arriving.
    assert_eq!(
        (decoded.width(), decoded.height()),
        (48, 36),
        "a 320x240 clip fits a 48px box at 48x36"
    );
}
