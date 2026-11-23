//! Media thumbnails are produced by the sandbox worker, not in this process.
//!
//! This is the one pipeline that executes an external program: a confinement
//! that fails here reads as "no thumbnail" rather than as a broken sandbox. A
//! host with no helper fails the test rather than passing it, so the file
//! authorization, the decode and the resize are measured, not described.

use crate::common;

use common::TestFixture;
use common::sandbox::{
    generate_clip, generate_large_clip, helper, helper_runs, tile_grid_frame_dimensions,
    without_ffmpeg,
};
use image::GenericImageView;
use server::sandbox::Profile;
use server::sandbox::worker::{self, Status};

/// Whether this host runs the media profile, with both layers that confine its
/// helper.
///
/// The media profile executes an external program, so it is the one profile that
/// needs the files and network layers on top of the resource bound. A host that
/// cannot establish them (a locked-down Windows host refuses the container) runs no
/// media thumbnails; the tests below report that and skip instead of failing, because
/// the answer is the policy working as intended.
fn media_profile_runs() -> bool {
    match worker::status(Profile::Media) {
        Status::Ready(report) => {
            assert!(
                report.protections.files && report.protections.network,
                "an accepted media report has to show both layers: {}",
                report.detail
            );
            true
        }
        Status::Unavailable(why) => {
            assert!(
                why.contains("files") && why.contains("network"),
                "the media profile was refused for an unexpected reason: {why}"
            );
            eprintln!("the media profile is unavailable on this host: {why}");
            false
        }
    }
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
    match worker::status(Profile::Media) {
        Status::Ready(report) => {
            assert!(
                report.protections.limits,
                "the media worker must bound itself: {}",
                report.detail
            );
            // It runs only with the layers that confine the helper, so an accepted
            // report has them.
            assert!(
                report.protections.files && report.protections.network,
                "the confined media worker must show both layers: {}",
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
        Status::Unavailable(why) => {
            // The expected shape on a host that cannot give the media profile the
            // layers its helper needs: no media thumbnails there, and the reason
            // names the layer.
            assert!(
                why.contains("files") && why.contains("network"),
                "the media sandbox was unavailable for an unexpected reason: {why}"
            );
            eprintln!("the media sandbox is unavailable on this host: {why}");
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
    if !media_profile_runs() {
        // The media profile is refused on this host, so media thumbnails are off —
        // and the request has to answer "no thumbnail" instead of failing the
        // upload or decoding in the server process.
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
            404,
            "a refused media profile must answer no thumbnail, not fail"
        );
        return;
    }
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
    assert_eq!(
        (decoded.width(), decoded.height()),
        (48, 36),
        "a 320x240 clip fits a 48px box at 48x36"
    );
}

/// A HEIC whose image is a HEIF tile grid (the shape an iPhone writes) gets a
/// whole-image thumbnail, not one tile. FFmpeg exposes the grid as a stream
/// group and rejects a simple `-vf` chain, so the worker must assemble it.
#[tokio::test]
async fn a_tiled_heic_thumbnail_is_the_whole_image() {
    let dir = tempfile::tempdir().expect("temp dir");
    let fixture = dir.path().join("tiled.heic");
    std::fs::write(&fixture, include_bytes!("fixtures/media/tiled.heic"))
        .expect("write the tiled HEIC fixture");
    if !helper_runs() {
        without_ffmpeg();
        return;
    }
    let full_grid = (192, 128);
    match tile_grid_frame_dimensions(&fixture) {
        Some(dimensions) if dimensions == full_grid => {}
        Some((width, height)) => {
            // The Linux CI image's ffmpeg 6.1 assembles one 64x64 tile instead
            // of the grid; Windows and macOS use a newer helper.
            eprintln!(
                "{} assembles {width}x{height}, not the full 192x128 grid; skipping",
                helper()
            );
            return;
        }
        None => {
            eprintln!(
                "{} cannot run the tile-grid graph; skipping the whole-image test",
                helper()
            );
            return;
        }
    }

    let f = TestFixture::new_with_media_helper(helper()).await;
    if !media_profile_runs() {
        return;
    }
    let bytes = std::fs::read(&fixture).expect("read the fixture");
    let resp = f
        .client
        .upload_file(&f.api_token, &f.repo_id, "/", "tiled.heic", &bytes)
        .await;
    assert!(resp.status().is_success(), "upload should succeed");

    let resp = f
        .client
        .get(
            &format!("/api2/repos/{}/thumbnail/?p=/tiled.heic&size=48", f.repo_id),
            Some(&f.api_token),
        )
        .await;
    assert_eq!(resp.status(), 200, "a tiled HEIC must get a thumbnail");
    let data = resp.bytes().await.expect("bytes");
    let decoded = image::load_from_memory(&data).expect("the reply is an image");
    assert_eq!(
        (decoded.width(), decoded.height()),
        (48, 32),
        "the 192x128 grid must fit a 48px box at 48x32, not one 64x64 tile at 48x48"
    );

    // The fixture is three coloured columns; a thumbnail of one tile would be a
    // single flat colour. Sampling the columns proves the grid was assembled.
    let left = decoded.get_pixel(4, 16).0;
    let right = decoded.get_pixel(44, 16).0;
    assert!(
        left[0].abs_diff(right[0]) > 40
            || left[1].abs_diff(right[1]) > 40
            || left[2].abs_diff(right[2]) > 40,
        "the thumbnail must span more than one tile: left={left:?} right={right:?}"
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
    if !media_profile_runs() {
        return;
    }
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
    if !media_profile_runs() {
        return;
    }
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
