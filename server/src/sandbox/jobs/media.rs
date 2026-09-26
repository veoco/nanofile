//! Media frame extraction, in the confined child.
//!
//! ffmpeg is an external program, so this is the one job that executes a helper.
//! The helper is the configured `storage.ffmpeg_path` and nothing else: the
//! parent names it on the child's command line, the child's own grants reach
//! exactly that file, and the request cannot substitute another.
//!
//! The parent writes the media to a scratch file first — ffmpeg has to seek a
//! container to find a frame — and hands over that one path. The child runs
//! ffmpeg with the frame on a pipe, decodes and resizes it, and answers with the
//! encoded thumbnail. Nothing of the media ever reaches the server process.
//!
//! The request is `NFX1`, one kind byte, a little-endian `u32` target size, a
//! little-endian `u16` path length and that many path bytes. The reply is the
//! common frame: `P`/`J` for an encoded image, `U` for a refusal.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::sandbox::worker::{self, RunOutcome};
use crate::sandbox::{Grants, Profile};
use crate::thumbnail_util::{self, ThumbFormat};

/// What the source is, which decides how ffmpeg is asked for a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A frame from the video stream (~1s in, first frame as fallback).
    Video,
    /// The embedded cover art (attached picture) of an audio file.
    Audio,
    /// A still image (HEIC/HEIF/AVIF) decoded by ffmpeg's image demuxer.
    Image,
}

impl Kind {
    fn as_byte(self) -> u8 {
        match self {
            Kind::Video => b'V',
            Kind::Audio => b'A',
            Kind::Image => b'I',
        }
    }

    fn from_byte(byte: u8) -> Option<Self> {
        Some(match byte {
            b'V' => Kind::Video,
            b'A' => Kind::Audio,
            b'I' => Kind::Image,
            _ => return None,
        })
    }
}

/// Most bytes of a frame the child will hold. A single frame of a video is at
/// most a few megabytes even before the resize; this bounds the pipe read.
const MAX_FRAME_BYTES: u64 = 64 * 1024 * 1024;

/// Most bytes of the source path in a request.
const MAX_PATH_BYTES: usize = 4096;

/// Extract a thumbnail for `source` with `ffmpeg`, in the confined child.
pub fn thumbnail(
    kind: Kind,
    source: &Path,
    size: u32,
    ffmpeg: &Path,
) -> Result<(Vec<u8>, ThumbFormat), String> {
    let request = request(kind, size, source)?;
    let grants = Grants {
        helper: Some(ffmpeg),
        source: Some(source),
    };
    match worker::run_request(Profile::Media, worker::MEDIA_TIMEOUT, request, grants) {
        RunOutcome::Reply { tag, body } => match tag {
            b'P' => Ok((body, ThumbFormat::Png)),
            b'J' => Ok((body, ThumbFormat::Jpeg)),
            b'U' => Err(String::from_utf8_lossy(&body).into_owned()),
            _ => Err(format!(
                "the media worker answered with an unknown tag {tag:?}"
            )),
        },
        RunOutcome::Unavailable(why) => Err(why),
        RunOutcome::Failed(why) => Err(why.to_string()),
    }
}

/// The request the parent sends.
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

/// The child's half of the protocol.
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

    // The child reads the one file it was granted, whatever the request says:
    // the path travels in the request only so the parent and the child agree on
    // which file that is, and a mismatch is a request this profile will not run.
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

    let (tag, body) = match grab_frame(helper, kind, &source) {
        Ok(frame) => match thumbnail_util::generate_thumbnail_encoded(&frame, size) {
            Ok((bytes, ThumbFormat::Png)) => (b'P', bytes),
            Ok((bytes, ThumbFormat::Jpeg)) => (b'J', bytes),
            Err(error) => (b'U', error.to_string().into_bytes()),
        },
        Err(why) => (b'U', why.into_bytes()),
    };
    worker::write_frame(tag, &body)
}

/// Run the helper once and hand back the PNG frame it wrote to its stdout.
///
/// Shared with the service's own tests, which drive the invocation without the
/// sandbox to check the arguments ffmpeg is given.
pub(crate) fn grab_frame(helper: &Path, kind: Kind, source: &Path) -> Result<Vec<u8>, String> {
    // Video is tried a second in before the first frame; a short clip has no
    // second to seek to, and that attempt failing is not a refusal.
    let attempts: Vec<Option<&str>> = match kind {
        Kind::Video => vec![Some("1"), None],
        Kind::Audio | Kind::Image => vec![None],
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
            // One frame, one thread: the child carries tight descriptor and
            // address-space limits, and ffmpeg's frame-threaded encoder asks for
            // more than those leave. The frame is a thumbnail, so there is
            // nothing to gain from asking for parallelism.
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
            .arg("pipe:1")
            // The protocol's own stdin is not the helper's to read.
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        match command.output() {
            Ok(output) if output.status.success() && !output.stdout.is_empty() => {
                if output.stdout.len() as u64 > MAX_FRAME_BYTES {
                    last = "the frame ffmpeg produced is over the size cap".to_string();
                    continue;
                }
                return Ok(output.stdout);
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                last = if stderr.trim().is_empty() {
                    "ffmpeg produced no frame".to_string()
                } else {
                    stderr.trim().to_string()
                };
            }
            Err(error) => return Err(format!("cannot run the media helper: {error}")),
        }
    }
    Err(last)
}

/// The command that starts the helper, with the path macOS needs.
///
/// The standard library starts a program with `posix_spawn` there, and fork+exec
/// is the path a literal `process-exec` grant is written for: an empty
/// `pre_exec` hook is what selects it. The hook itself must be
/// async-signal-safe, and doing nothing is.
///
/// Both paths report the same refusal on macOS today — see the module docs on
/// what this profile does not yet do on that platform — so this is not what
/// stands between the media worker and a helper there.
fn helper_command(helper: &Path) -> Command {
    #[allow(unused_mut)]
    let mut command = Command::new(helper);
    #[cfg(target_os = "macos")]
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(|| Ok(()));
    }
    command
}

/// The self-test: run the helper under the profile.
///
/// `-version` proves the grant reaches the binary and its libraries. It does not
/// decode anything, which is what the first real request is for.
pub fn probe(grants: Grants<'_>) -> String {
    let Some(helper) = grants.helper else {
        return "media-no-helper".to_string();
    };
    match helper_command(helper)
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(status) if status.success() => "media-ok".to_string(),
        Ok(status) => format!("media-failed({status})"),
        Err(error) => format!("media-unavailable({error})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The kind survives the wire, and nothing else does.
    #[test]
    fn a_kind_round_trips() {
        for kind in [Kind::Video, Kind::Audio, Kind::Image] {
            assert_eq!(Kind::from_byte(kind.as_byte()), Some(kind));
        }
        assert_eq!(Kind::from_byte(b'?'), None);
    }

    /// The request names the operation, the size and the source path.
    #[test]
    fn the_request_carries_the_source_path() {
        let request = request(Kind::Video, 48, Path::new("/tmp/a.bin")).expect("request");
        assert_eq!(&request[..4], worker::MAGIC);
        assert_eq!(request[4], b'V');
        assert_eq!(u32::from_le_bytes(request[5..9].try_into().unwrap()), 48);
        assert_eq!(u16::from_le_bytes(request[9..11].try_into().unwrap()), 10);
        assert_eq!(&request[11..], b"/tmp/a.bin");
    }

    /// A source that is not UTF-8 is refused before anything is spawned.
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
