//! Face detection for the automatic people tags, in its own confined child.
//!
//! The child loads the detector from the binary and answers with a count, never
//! with pixels: the parent only ever sees the JSON summary (count + best score).
//! It is a profile of its own so a detector that fails — at startup, under load,
//! or on one input — cannot take image work down with it. The image worker's
//! probe carries no inference for the same reason (see
//! [`super::images::probe`]).
//!
//! HEIC/HEIF/AVIF never reach this job: the `image` crate cannot decode them, so
//! the caller decodes a still frame through the media profile first and sends
//! that.
//!
//! Request: `NFX1`, one operation byte, a little-endian `u32` packed options
//! word, then the image's bytes. Reply: the common frame, `J` for the JSON
//! summary, `R` for bytes this build cannot decode (a verdict, not a failure) and
//! `U` for a run that failed and is worth retrying.

use std::io::Read;

use crate::face_tags::DetectOptions;
use crate::face_tags::engine::DetectionSummary;
use crate::sandbox::worker::{self, RunOutcome};
use crate::sandbox::{Grants, JobVerdict, Profile};

/// Detect faces, which is this profile's only operation.
pub const OP_DETECT: u8 = b'D';

/// Why a face scan produced no count.
///
/// The split is what decides whether the caller retries: a decoder that refuses
/// these bytes refuses them next hour too, while a child that failed to start or a
/// detector that ran out of memory is worth another attempt.
#[derive(Debug)]
pub(crate) enum FaceError {
    /// A verdict on the bytes: this build cannot decode them (over the dimension
    /// limit, a truncated container, a format the decoder does not read).
    Refused(String),
    /// The environment failed: retry later.
    Retry(String),
}

impl FaceError {
    /// The reply frame for a child-side failure: the tag says which half it is.
    fn into_reply(self) -> (u8, Vec<u8>) {
        match self {
            Self::Refused(reason) => (b'R', reason.into_bytes()),
            Self::Retry(reason) => (b'U', reason.into_bytes()),
        }
    }
}

/// Most image bytes the child will accept: the parent already caps what it reads
/// (32 MiB), and this is the second half of the same bound.
pub(crate) const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;

/// The gate that bounds how many face children are alive at once.
///
/// Separate from the image gate on purpose: a queue of thumbnails must not starve
/// face scans, and a slow detector must not hold an image permit. A constant, so it
/// cannot be set unlimited.
pub(crate) const MAX_CONCURRENT_FACE_WORKERS: usize = 2;

static FACE_CONCURRENCY: std::sync::OnceLock<std::sync::Arc<tokio::sync::Semaphore>> =
    std::sync::OnceLock::new();

/// Acquire a permit covering one face-worker child; callers queue rather than fail.
pub(crate) async fn acquire_face_permit() -> Result<tokio::sync::OwnedSemaphorePermit, String> {
    FACE_CONCURRENCY
        .get_or_init(|| {
            std::sync::Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_FACE_WORKERS))
        })
        .clone()
        .acquire_owned()
        .await
        .map_err(|error| format!("face concurrency gate failed: {error}"))
}

/// Ask the worker for the face count. The reply is deliberately tiny: no boxes,
/// crops or embeddings leave the child.
pub(crate) fn detect(
    content: &[u8],
    options: DetectOptions,
) -> Result<DetectionSummary, FaceError> {
    let request = request(OP_DETECT, encode_options(options), content);
    match worker::run_request(
        Profile::Faces,
        worker::FACE_TIMEOUT,
        request,
        Grants::default(),
    ) {
        RunOutcome::Reply { tag, body } => reply(tag, &body),
        RunOutcome::Unavailable(why) => Err(FaceError::Retry(why)),
        RunOutcome::Failed(why) => Err(FaceError::Retry(why.to_string())),
    }
}

/// Read one reply frame from the child: the tag says whether the bytes were
/// refused or the run failed.
fn reply(tag: u8, body: &[u8]) -> Result<DetectionSummary, FaceError> {
    match tag {
        b'J' => {
            let reply: Reply = serde_json::from_slice(body).map_err(|e| {
                FaceError::Retry(format!("the face worker answered invalid JSON: {e}"))
            })?;
            Ok(DetectionSummary {
                face_count: reply.face_count,
                max_score: reply.max_score,
            })
        }
        b'R' => Err(FaceError::Refused(
            String::from_utf8_lossy(body).into_owned(),
        )),
        b'U' => Err(FaceError::Retry(String::from_utf8_lossy(body).into_owned())),
        _ => Err(FaceError::Retry(format!(
            "the face worker answered with an unknown tag {tag:?}"
        ))),
    }
}

#[derive(serde::Deserialize)]
struct Reply {
    face_count: usize,
    max_score: f32,
}

fn request(op: u8, options: u32, content: &[u8]) -> Vec<u8> {
    let mut request = Vec::with_capacity(worker::MAGIC.len() + 5 + content.len());
    request.extend_from_slice(worker::MAGIC);
    request.push(op);
    request.extend_from_slice(&options.to_le_bytes());
    request.extend_from_slice(content);
    request
}

pub fn serve() -> anyhow::Result<()> {
    let mut stdin = std::io::stdin().lock();
    let mut head = [0u8; 9];
    stdin.read_exact(&mut head)?;
    if &head[..4] != worker::MAGIC {
        anyhow::bail!("the request does not start with the protocol magic");
    }
    let op = head[4];
    let options = u32::from_le_bytes(head[5..9].try_into().expect("four bytes"));

    let mut data = Vec::new();
    stdin.read_to_end(&mut data)?;
    if data.len() > MAX_IMAGE_BYTES {
        return worker::write_frame(b'R', b"image over the size cap");
    }

    let (tag, body) = answer(op, decode_options(options), &data);
    worker::write_frame(tag, &body)
}

/// Do the work one operation names, as a reply frame's tag and body.
fn answer(op: u8, options: DetectOptions, data: &[u8]) -> (u8, Vec<u8>) {
    match op {
        OP_DETECT => match detect_answer(data, options) {
            Ok(reply) => (
                b'J',
                serde_json::to_vec(&reply).unwrap_or_else(|_| b"null".to_vec()),
            ),
            Err(error) => error.into_reply(),
        },
        _ => (b'U', b"unknown face operation".to_vec()),
    }
}

fn detect_answer(data: &[u8], options: DetectOptions) -> Result<DetectionSummary, FaceError> {
    // A decoder refusal is a verdict on the bytes: the same file will not decode
    // next hour either, so it is reported apart from a run that failed.
    let image = crate::thumbnail_util::load_image_with_orientation(data)
        .map_err(|error| FaceError::Refused(decode_detail(error)))?;
    crate::face_tags::engine::detect(&image, options).map_err(FaceError::Retry)
}

/// The readable half of a decode failure: its payload already says what failed, and
/// the "internal error" prefix means "retry me", which these bytes never will.
fn decode_detail(error: base::error::AppError) -> String {
    match error {
        base::error::AppError::Internal(detail) => detail,
        other => other.to_string(),
    }
}

fn encode_options(options: DetectOptions) -> u32 {
    let score = (options.min_score.clamp(0.0, 1.0) * 1000.0).round() as u32;
    let size = options.min_face_size.clamp(0.0, 65535.0) as u32;
    (score << 16) | size
}

fn decode_options(raw: u32) -> DetectOptions {
    if raw == 0 {
        return DetectOptions::default();
    }
    let mut options = DetectOptions {
        min_score: ((raw >> 16) as f32 / 1000.0).clamp(0.0, 1.0),
        ..Default::default()
    };
    let size = raw & 0xffff;
    if size > 0 {
        options.min_face_size = size as f32;
    }
    options
}

/// The self-test: load the detector and run one forward pass, so a profile that
/// cannot do the work says so here rather than on every scan.
///
/// The image is generated, so the probe has no fixture to lose; a blank one still
/// runs the whole pipeline the detector needs (letterbox, tensor build, forward,
/// decode). That the model *finds* a face is [`crate::face_tags::engine`]'s unit
/// test, which runs the real portrait; this answers "the confined child can run
/// the detector at all", which is what a startup probe is for.
pub fn probe() -> (String, JobVerdict) {
    let image =
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 2, image::Rgb([0, 0, 0])));
    match crate::face_tags::engine::detect(&image, DetectOptions::default()) {
        Ok(summary) => (format!("faces-ok({})", summary.face_count), JobVerdict::Ok),
        Err(error) => (
            format!("faces-failed({})", token(&error)),
            JobVerdict::Failed,
        ),
    }
}

/// One token out of a failure message: the detail is comma-separated and
/// whitespace-free.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_is_what_the_child_reads() {
        let request = request(
            OP_DETECT,
            encode_options(DetectOptions::default()),
            b"payload",
        );
        assert_eq!(&request[..4], worker::MAGIC);
        assert_eq!(request[4], OP_DETECT);
        assert_eq!(
            u32::from_le_bytes(request[5..9].try_into().unwrap()),
            encode_options(DetectOptions::default())
        );
        assert_eq!(&request[9..], b"payload");
    }

    #[test]
    fn the_options_survive_the_wire() {
        let options = DetectOptions {
            min_score: 0.42,
            min_face_size: 48.0,
        };
        let decoded = decode_options(encode_options(options));
        assert!((decoded.min_score - 0.42).abs() < 0.001, "{decoded:?}");
        assert_eq!(decoded.min_face_size, 48.0);
        // Zero is the "use the defaults" word, not a zero threshold.
        let defaults = decode_options(0);
        assert_eq!(defaults.min_score, DetectOptions::default().min_score);
        assert_eq!(
            defaults.min_face_size,
            DetectOptions::default().min_face_size
        );
    }

    #[test]
    fn a_request_without_a_face_answers_a_count_not_pixels() {
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([1, 2, 3])))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("PNG");
        let (tag, body) = answer(OP_DETECT, DetectOptions::default(), &png);
        assert_eq!(tag, b'J');
        let value: serde_json::Value = serde_json::from_slice(&body).expect("JSON");
        assert_eq!(value["face_count"], 0);
    }

    /// Undecodable bytes are a verdict, not a failure: the people-tag backfill used
    /// to retry them every hour for as long as the server ran.
    #[test]
    fn undecodable_bytes_are_a_refusal() {
        let (tag, body) = answer(OP_DETECT, DetectOptions::default(), b"not an image");
        assert_eq!(tag, b'R');
        assert!(
            !String::from_utf8_lossy(&body).contains("internal error"),
            "a refusal carries the reason, not a retryable failure's prefix"
        );
    }

    #[test]
    fn a_refusal_and_a_failure_are_told_apart_on_the_wire() {
        assert!(matches!(
            reply(b'R', b"decoder refused"),
            Err(FaceError::Refused(reason)) if reason == "decoder refused"
        ));
        assert!(matches!(
            reply(b'U', b"detector failed"),
            Err(FaceError::Retry(reason)) if reason == "detector failed"
        ));
        assert!(matches!(
            reply(b'J', br#"{"face_count":2,"max_score":0.75}"#),
            Ok(summary) if summary.face_count == 2
        ));
    }

    #[test]
    fn an_unknown_operation_is_refused() {
        let (tag, body) = answer(b'?', DetectOptions::default(), b"");
        assert_eq!(tag, b'U');
        assert_eq!(
            std::str::from_utf8(&body).unwrap(),
            "unknown face operation"
        );
    }

    #[test]
    fn the_probe_runs_the_detector() {
        let (token, job) = probe();
        assert_eq!(token, "faces-ok(0)");
        assert_eq!(job, JobVerdict::Ok);
    }
}
