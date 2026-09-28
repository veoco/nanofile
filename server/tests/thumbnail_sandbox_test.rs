//! Media thumbnails are produced by the sandbox worker, not in this process.
//!
//! This is the one pipeline that executes an external program: a confinement
//! that fails here reads as "no thumbnail" rather than as a broken sandbox. A
//! host with no helper fails the test rather than passing it, so the file
//! authorization, the decode and the resize are measured, not described.

mod common;

use common::TestFixture;
use common::sandbox::{generate_clip, generate_large_clip, helper, helper_runs, without_ffmpeg};

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

/// A video past the memory-only head cap still gets a thumbnail when its index
/// sits at the front: only the prefix is read, so a multi-hundred-MB upload never
/// hits the old whole-file 512 MiB cap (which used to 404 here).
#[tokio::test]
async fn a_large_faststart_video_thumbnail_reads_only_the_head_in_the_sandbox() {
    let dir = tempfile::tempdir().expect("temp dir");
    let clip = dir.path().join("large.mov");
    if !generate_large_clip(&clip, true) {
        without_ffmpeg();
        return;
    }
    let bytes = std::fs::read(&clip).expect("read the clip");
    assert!(
        bytes.len() as u64 > 32 * 1024 * 1024,
        "the fixture must exceed the media head cap to exercise the head path"
    );

    let f = TestFixture::new_with_media_helper(helper()).await;
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "large.mov", &bytes)
        .await;
    assert!(resp.status().is_success(), "upload should succeed");

    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/thumbnail/?p=/large.mov&size=48", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(
        resp.status(),
        200,
        "a large faststart video must get a thumbnail from a read head"
    );
    let data = resp.bytes().await.expect("bytes");
    let decoded = image::load_from_memory(&data).expect("the reply is an image");
    assert_eq!(
        (decoded.width(), decoded.height()),
        (48, 36),
        "the head-only frame must still decode and resize"
    );
}

/// A non-faststart video whose index trails the data keeps working: the head
/// read misses, and because the file fits the 512 MiB cap the whole file is read
/// once as a fallback.
#[tokio::test]
async fn a_large_non_faststart_video_thumbnail_falls_back_to_the_whole_file_in_the_sandbox() {
    let dir = tempfile::tempdir().expect("temp dir");
    let clip = dir.path().join("tail.mov");
    if !generate_large_clip(&clip, false) {
        without_ffmpeg();
        return;
    }
    let bytes = std::fs::read(&clip).expect("read the clip");
    assert!(
        bytes.len() as u64 > 32 * 1024 * 1024,
        "the fixture must exceed the media head cap to force the fallback"
    );

    let f = TestFixture::new_with_media_helper(helper()).await;
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "tail.mov", &bytes)
        .await;
    assert!(resp.status().is_success(), "upload should succeed");

    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/thumbnail/?p=/tail.mov&size=48", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(
        resp.status(),
        200,
        "a non-faststart video within the cap must still get a thumbnail"
    );
}
