//! Media frame extraction, in the confined child.
//!
//! ffmpeg is an external program, so this is the one job that executes a helper. The helper
//! is the configured `storage.ffmpeg_path` and nothing else: the parent names it on the
//! child's command line, the child's grants reach exactly that file, and the request cannot
//! substitute another.
//!
//! The parent writes the media to a scratch file — ffmpeg has to seek a container — and hands
//! over that path; the child decodes a frame and answers with the encoded thumbnail. The
//! `Still` kind is the same run without the resize: the people-tag scan asks for one decodable
//! frame of a HEIC/HEIF/AVIF image and hands it to the detector's own worker.
//!
//! Request: `NFX1`, one kind byte, a little-endian `u32` target size, a little-endian `u16`
//! path length and that many path bytes. Reply: the common frame, `P`/`J` for an image, `E`
//! when the environment refused and `U` when the file did.

// `unsafe` is the macOS `pre_exec` hook and the fork-based test helper; the module
// doc explains why the helper must fork rather than `posix_spawn`.
#![cfg_attr(target_os = "macos", allow(unsafe_code))]

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::sandbox::worker::{self, RunOutcome};
use crate::sandbox::{Grants, JobVerdict, Profile};
use crate::thumbnail_util::{self, ThumbFormat};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A frame from the video stream (~1s in, first frame as fallback).
    Video,
    /// The embedded cover art (attached picture) of an audio file.
    Audio,
    /// A still image (HEIC/HEIF/AVIF) decoded by ffmpeg's image demuxer, for a
    /// thumbnail.
    Image,
    /// A still image decoded for the face detector: no thumbnail resize, and its
    /// own, larger size ceiling.
    Still,
}

impl Kind {
    fn as_byte(self) -> u8 {
        match self {
            Kind::Video => b'V',
            Kind::Audio => b'A',
            Kind::Image => b'I',
            Kind::Still => b'S',
        }
    }

    fn from_byte(byte: u8) -> Option<Self> {
        Some(match byte {
            b'V' => Kind::Video,
            b'A' => Kind::Audio,
            b'I' => Kind::Image,
            b'S' => Kind::Still,
            _ => return None,
        })
    }
}

/// Most bytes of a frame the child will hold. A single frame of a video is at
/// most a few megabytes even before the resize; this bounds the pipe read.
const MAX_FRAME_BYTES: u64 = 64 * 1024 * 1024;

/// Most bytes of a still decoded for the detector: the parent's reply cap is 9 MiB,
/// and a JPEG at 4096 px stays far below this.
const MAX_STILL_BYTES: u64 = 8 * 1024 * 1024;

/// The long edge a still is decoded to.
///
/// It matches what the detector's own source cap accepts (8192 is the image crate's
/// bound; 4096 keeps a phone photo at its true pixels and still bounds the JPEG) and
/// keeps the parent's reply under its cap.
const STILL_MAX_EDGE: u32 = 4096;

const MAX_PATH_BYTES: usize = 4096;

/// The media the self-test decodes: two seconds of a 64×64 test pattern.
///
/// The child has nothing to open, so it travels in the binary; it is checked in because the
/// profile is measured on hosts with no fixture tooling, and encoded with the native MPEG-4
/// codec every build of `ffmpeg` carries. Regenerate with:
///
/// ```text
/// ffmpeg -y -f lavfi -i testsrc=duration=2:size=64x64:rate=5 \
///   -pix_fmt yuv420p -c:v mpeg4 -q:v 5 server/tests/fixtures/probe.mp4
/// ```
pub(crate) const PROBE_MEDIA: &[u8] = include_bytes!("../../../tests/fixtures/probe.mp4");

const PROBE_SIZE: u32 = 48;

/// The path a media grant and a media request must agree on.
///
/// A Seatbelt `(literal …)` is written for the resolved file and the child must open that
/// same path: `TMPDIR` is reached through the `/var` symlink on every Mac, so a grant
/// written from the typed path would not match. Elsewhere the path is returned as it was —
/// Landlock rules are on the object, and Windows `canonicalize` returns a `\\?\` path.
pub(crate) fn granted_source(source: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        std::fs::canonicalize(source).unwrap_or_else(|_| source.to_path_buf())
    }
    #[cfg(not(target_os = "macos"))]
    {
        source.to_path_buf()
    }
}

pub fn thumbnail(
    kind: Kind,
    source: &Path,
    size: u32,
    ffmpeg: &Path,
) -> Result<(Vec<u8>, ThumbFormat), String> {
    let source = granted_source(source);
    let request = request(kind, size, &source)?;
    let grants = Grants {
        helper: Some(ffmpeg),
        source: Some(&source),
    };
    match worker::run_request(Profile::Media, worker::MEDIA_TIMEOUT, request, grants) {
        RunOutcome::Reply { tag, body } => match tag {
            b'P' => Ok((body, ThumbFormat::Png)),
            b'J' => Ok((body, ThumbFormat::Jpeg)),
            // `E` is the environment and `U` the file; a thumbnail treats both as
            // "no thumbnail", the caller that cares uses `still`.
            b'U' | b'E' => Err(String::from_utf8_lossy(&body).into_owned()),
            _ => Err(format!(
                "the media worker answered with an unknown tag {tag:?}"
            )),
        },
        RunOutcome::Unavailable(why) => Err(why),
        RunOutcome::Failed(why) => Err(why.to_string()),
    }
}

/// Why a still decode produced no frame, kept apart because the caller records them
/// differently: an environment problem is retried, a verdict on the bytes is not.
#[derive(Debug)]
pub(crate) enum StillError {
    /// The media profile or its helper could not run: retry later.
    Unavailable(String),
    /// The child refused the file itself: too large, or not something its ffmpeg can
    /// decode. A verdict on the bytes, not retried.
    Refused(String),
    /// The child died, timed out, or answered with something else.
    Failed(String),
}

/// Decode one still image through the media profile, for the detector.
///
/// The answer is JPEG at up to [`STILL_MAX_EDGE`] on its long edge — the source the
/// face worker decodes. HEIC/HEIF/AVIF are the formats the `image` crate cannot read.
pub(crate) fn still(source: &Path, ffmpeg: &Path) -> Result<Vec<u8>, StillError> {
    let source = granted_source(source);
    let request = request(Kind::Still, 0, &source).map_err(StillError::Failed)?;
    let grants = Grants {
        helper: Some(ffmpeg),
        source: Some(&source),
    };
    match worker::run_request(Profile::Media, worker::MEDIA_TIMEOUT, request, grants) {
        RunOutcome::Reply { tag, body } => match tag {
            b'J' => Ok(body),
            b'E' => Err(StillError::Unavailable(
                String::from_utf8_lossy(&body).into_owned(),
            )),
            b'U' => Err(StillError::Refused(
                String::from_utf8_lossy(&body).into_owned(),
            )),
            _ => Err(StillError::Failed(format!(
                "the media worker answered with an unknown tag {tag:?}"
            ))),
        },
        RunOutcome::Unavailable(why) => Err(StillError::Unavailable(why)),
        RunOutcome::Failed(why) => Err(StillError::Failed(why.to_string())),
    }
}

fn request(kind: Kind, size: u32, source: &Path) -> Result<Vec<u8>, String> {
    let path = source
        .to_str()
        .ok_or_else(|| "the media source path is not UTF-8".to_string())?
        .as_bytes();
    if path.len() > MAX_PATH_BYTES || path.len() > u16::MAX as usize {
        return Err("the media source path is too long".to_string());
    }
    let mut request = Vec::with_capacity(worker::MAGIC.len() + 7 + path.len());
    request.extend_from_slice(worker::MAGIC);
    request.push(kind.as_byte());
    request.extend_from_slice(&size.to_le_bytes());
    request.extend_from_slice(&(path.len() as u16).to_le_bytes());
    request.extend_from_slice(path);
    Ok(request)
}

pub fn serve(grants: Grants<'_>) -> anyhow::Result<()> {
    let mut stdin = std::io::stdin().lock();
    let mut head = [0u8; 11];
    stdin.read_exact(&mut head)?;
    if &head[..4] != worker::MAGIC {
        anyhow::bail!("the request does not start with the protocol magic");
    }
    let Some(kind) = Kind::from_byte(head[4]) else {
        return worker::write_frame(b'U', b"unknown media kind");
    };
    let size = u32::from_le_bytes(head[5..9].try_into().expect("four bytes"));
    let path_len = u16::from_le_bytes(head[9..11].try_into().expect("two bytes")) as usize;
    if path_len == 0 || path_len > MAX_PATH_BYTES {
        return worker::write_frame(b'U', b"bad media source path length");
    }

    let mut path = vec![0u8; path_len];
    stdin.read_exact(&mut path)?;
    let source = PathBuf::from(
        String::from_utf8(path)
            .map_err(|_| anyhow::anyhow!("the media source path is not UTF-8"))?,
    );

    // The path travels in the request only so the two sides agree on the file; a
    // mismatch is a request this profile will not run.
    let Some(granted) = grants.source else {
        return worker::write_frame(b'U', b"the media profile has no source grant");
    };
    if source != granted {
        return worker::write_frame(
            b'U',
            b"the request names a source this profile was not granted",
        );
    }
    let Some(helper) = grants.helper else {
        return worker::write_frame(b'U', b"the media profile has no helper grant");
    };

    let (tag, body) = match kind {
        Kind::Still => match still_frame(helper, &source) {
            Ok(bytes) => (b'J', bytes),
            Err(FrameError::Unavailable(why)) => (b'E', why.into_bytes()),
            Err(FrameError::Failed(why)) => (b'U', why.into_bytes()),
        },
        _ => match encoded_thumbnail(helper, kind, &source, size) {
            Ok((bytes, ThumbFormat::Png)) => (b'P', bytes),
            Ok((bytes, ThumbFormat::Jpeg)) => (b'J', bytes),
            Err(why) => (b'U', why.message().as_bytes().to_vec()),
        },
    };
    worker::write_frame(tag, &body)
}

/// Grab one frame and encode the thumbnail a request answers with.
///
/// [`serve`] and the self-test both run this call, so the probe measures the work a request
/// does, not a helper that merely starts.
fn encoded_thumbnail(
    helper: &Path,
    kind: Kind,
    source: &Path,
    size: u32,
) -> Result<(Vec<u8>, ThumbFormat), FrameError> {
    let frame = grab_frame(helper, kind, source)?;
    thumbnail_util::generate_thumbnail_encoded(&frame, size)
        .map_err(|error| FrameError::Failed(error.to_string()))
}

/// Why the helper handed over no frame: the environment (never ran) or the input (ran and
/// produced nothing usable). The report keeps them apart; a refusal is a refusal.
#[derive(Debug)]
pub(crate) enum FrameError {
    Unavailable(String),
    Failed(String),
}

impl FrameError {
    fn message(&self) -> &str {
        match self {
            FrameError::Unavailable(why) | FrameError::Failed(why) => why,
        }
    }
}

/// Run the helper once and hand back the PNG frame it wrote to stdout.
pub(crate) fn grab_frame(helper: &Path, kind: Kind, source: &Path) -> Result<Vec<u8>, FrameError> {
    // Video is tried a second in before the first frame; a short clip has no
    // second to seek to, and that attempt failing is not a refusal.
    let attempts: Vec<Option<&str>> = match kind {
        Kind::Video => vec![Some("1"), None],
        Kind::Audio | Kind::Image | Kind::Still => vec![None],
    };

    let mut last = String::from("ffmpeg produced no frame");
    for seek in attempts {
        let mut command = helper_command(helper);
        command
            .arg("-y")
            .arg("-loglevel")
            .arg("error")
            .arg("-hide_banner");
        if let Some(seek) = seek {
            command.arg("-ss").arg(seek);
        }
        command.arg("-i").arg(source);
        if kind == Kind::Audio {
            command.arg("-map").arg("0:v:0");
        }
        command
            .arg("-frames:v")
            .arg("1")
            // One frame, one thread: frame threading asks for more than the child's limits leave.
            .arg("-threads")
            .arg("1")
            .arg("-filter_threads")
            .arg("1")
            .arg("-vf")
            .arg("scale=min(1024\\,iw):-2")
            .arg("-f")
            .arg("image2")
            .arg("-c:v")
            .arg("png")
            .arg("pipe:1");
        match run_capped(&mut command, MAX_FRAME_BYTES) {
            Ok(bytes) => return Ok(bytes),
            // The environment is the same answer for the next attempt.
            Err(FrameError::Unavailable(why)) => return Err(FrameError::Unavailable(why)),
            Err(FrameError::Failed(why)) => last = why,
        }
    }
    Err(FrameError::Failed(last))
}

/// Decode one still for the detector: no thumbnail resize, a JPEG, and a ceiling the
/// parent's reply pipe can carry.
///
/// The scale filter keeps a very large photo inside [`STILL_MAX_EDGE`]; the detector
/// sees the decoded image as its source, so that bound is part of the tag's meaning.
fn still_frame(helper: &Path, source: &Path) -> Result<Vec<u8>, FrameError> {
    let mut command = helper_command(helper);
    command
        .arg("-y")
        .arg("-loglevel")
        .arg("error")
        .arg("-hide_banner")
        .arg("-i")
        .arg(source)
        .arg("-frames:v")
        .arg("1")
        .arg("-threads")
        .arg("1")
        .arg("-filter_threads")
        .arg("1")
        .arg("-vf")
        .arg(format!("scale=min({STILL_MAX_EDGE}\\,iw):-2"))
        .arg("-f")
        .arg("image2pipe")
        .arg("-c:v")
        .arg("mjpeg")
        .arg("-q:v")
        .arg("3")
        .arg("pipe:1");
    run_capped(&mut command, MAX_STILL_BYTES)
}

/// Start the helper with the standard stream setup and collect a bounded stdout.
///
/// The helper's stdin is a pipe this side closes at once, not the null device: a
/// confinement layer decides whether that device may open, and both Seatbelt and the
/// Windows container have refused it.
fn run_capped(command: &mut Command, cap: u64) -> Result<Vec<u8>, FrameError> {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    match run_helper(command) {
        Ok(output) if output.status.success() && !output.stdout.is_empty() => {
            if output.stdout.len() as u64 > cap {
                return Err(FrameError::Failed(
                    "the frame ffmpeg produced is over the size cap".to_string(),
                ));
            }
            Ok(output.stdout)
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(FrameError::Failed(if stderr.trim().is_empty() {
                "ffmpeg produced no frame".to_string()
            } else {
                stderr.trim().to_string()
            }))
        }
        Err(error) => Err(FrameError::Unavailable(format!(
            "cannot run the media helper: {error}"
        ))),
    }
}

/// The command that starts the helper.
///
/// The standard library uses `posix_spawn` on macOS, which a Seatbelt `process-exec` literal
/// does not allow, so an empty `pre_exec` hook makes the library fork and exec instead — the
/// operation the grant is written for, and async-signal-safe because it does nothing.
fn helper_command(helper: &Path) -> Command {
    // `mut` is only used by the macOS `pre_exec` hook below.
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
    let mut command = Command::new(helper);
    #[cfg(target_os = "macos")]
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(|| Ok(()));
    }
    command
}

/// Run the helper with its streams on pipes and stdin closed: `Command::output` would set
/// stdin to the null device a confinement layer may refuse.
fn run_helper(command: &mut Command) -> std::io::Result<std::process::Output> {
    let mut child = command.spawn()?;
    drop(child.stdin.take());
    child.wait_with_output()
}

/// The self-test: decode a frame under the profile, the way a request does — the other half
/// of the answer beside the grant reaching the helper.
pub fn probe(grants: Grants<'_>) -> (String, JobVerdict) {
    let Some(helper) = grants.helper else {
        return ("media-no-helper".to_string(), JobVerdict::Nothing);
    };
    let Some(source) = grants.source else {
        // The parent could not write the probe's own file. That says nothing
        // about the helper, so it is not a failure of the profile.
        return ("media-no-source".to_string(), JobVerdict::Nothing);
    };
    let (token_text, job) = match encoded_thumbnail(helper, Kind::Video, source, PROBE_SIZE) {
        Ok((bytes, _)) => (format!("media-ok({})", bytes.len()), JobVerdict::Ok),
        Err(FrameError::Unavailable(why)) => (
            format!("media-unavailable({})", token(&why)),
            JobVerdict::Failed,
        ),
        Err(FrameError::Failed(why)) => {
            (format!("media-failed({})", token(&why)), JobVerdict::Failed)
        }
    };
    #[cfg(target_os = "macos")]
    let token_text = format!("{token_text},hand={}", hand_start(helper));
    (token_text, job)
}

/// One token out of a failure message: the detail is comma-separated and whitespace-free.
fn token(why: &str) -> String {
    why.chars()
        .map(|character| {
            if character.is_whitespace() || character == ',' {
                '_'
            } else {
                character
            }
        })
        .collect()
}

/// Start the helper by hand, and say what the kernel answered.
///
/// In the child that is failing: `fork`, `execve` the helper with `-version`, output on
/// `/dev/null`. `ok` means the grant is fine and the library's spawn path fails;
/// `exec=<errno>` means the sandbox refuses this child.
#[cfg(target_os = "macos")]
fn hand_start(helper: &Path) -> String {
    use std::ffi::CString;
    use std::os::fd::AsRawFd as _;
    use std::os::unix::ffi::OsStrExt as _;

    /// The errno, offset so it cannot be confused with the helper's exit code.
    const EXEC_FAILED: i32 = 100;

    let Ok(program) = CString::new(helper.as_os_str().as_bytes()) else {
        return "bad-path".to_string();
    };
    let Ok(sink) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/null")
    else {
        return "no-devnull".to_string();
    };

    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return format!(
            "fork={}",
            std::io::Error::last_os_error()
                .raw_os_error()
                .unwrap_or_default()
        );
    }
    if pid == 0 {
        unsafe {
            let fd = sink.as_raw_fd();
            libc::dup2(fd, libc::STDOUT_FILENO);
            libc::dup2(fd, libc::STDERR_FILENO);
            let version = c"-version";
            let argv = [program.as_ptr(), version.as_ptr(), std::ptr::null()];
            let envp = [std::ptr::null()];
            libc::execve(program.as_ptr(), argv.as_ptr(), envp.as_ptr());
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            libc::_exit(EXEC_FAILED + (errno & 0x7f));
        }
    }

    let mut status = 0;
    if unsafe { libc::waitpid(pid, &mut status, 0) } < 0 {
        return "wait".to_string();
    }
    if !libc::WIFEXITED(status) {
        return "signalled".to_string();
    }
    let code = libc::WEXITSTATUS(status);
    if code == 0 {
        "ok".to_string()
    } else if code >= EXEC_FAILED {
        format!("exec={}", code - EXEC_FAILED)
    } else {
        format!("exited={code}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kind_round_trips() {
        for kind in [Kind::Video, Kind::Audio, Kind::Image, Kind::Still] {
            assert_eq!(Kind::from_byte(kind.as_byte()), Some(kind));
        }
        assert_eq!(Kind::from_byte(b'?'), None);
    }

    /// The detector's source: a JPEG, at the still's own edge, decoded by the helper
    /// under the media profile rather than by the server.
    #[test]
    fn a_still_decodes_to_a_jpeg() {
        let dir = tempfile::tempdir().expect("temp dir");
        let source = dir.path().join("source.png");
        // A generated image, so the test needs no fixture; ffmpeg is on every CI
        // runner and its PNG decoder is part of the build.
        let mut image = image::RgbImage::new(64, 32);
        for pixel in image.pixels_mut() {
            *pixel = image::Rgb([30, 60, 90]);
        }
        image.save(&source).expect("write the source");
        let helper = Path::new("ffmpeg");
        if std::process::Command::new(helper)
            .arg("-version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| !status.success())
            .unwrap_or(true)
        {
            // A host with no helper: the integration tests skip the same way.
            return;
        }

        let frame = still_frame(helper, &source).expect("a still frame");
        assert_eq!(&frame[..2], &[0xFF, 0xD8], "a still is a JPEG");
        let decoded = image::load_from_memory(&frame).expect("the still decodes");
        assert_eq!((decoded.width(), decoded.height()), (64, 32));
    }

    #[test]
    fn the_request_carries_the_source_path() {
        let request = request(Kind::Video, 48, Path::new("/tmp/a.bin")).expect("request");
        assert_eq!(&request[..4], worker::MAGIC);
        assert_eq!(request[4], b'V');
        assert_eq!(u32::from_le_bytes(request[5..9].try_into().unwrap()), 48);
        assert_eq!(u16::from_le_bytes(request[9..11].try_into().unwrap()), 10);
        assert_eq!(&request[11..], b"/tmp/a.bin");
    }

    #[test]
    fn a_non_utf8_source_is_refused() {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let bad = Path::new(std::ffi::OsStr::from_bytes(b"/tmp/\xff"));
            assert!(request(Kind::Video, 48, bad).is_err());
        }
    }
}
