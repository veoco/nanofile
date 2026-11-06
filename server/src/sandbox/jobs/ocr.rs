//! OCR for the full-text index, in its own confined child.
//!
//! The child loads the page and line orientation, detection and recognition
//! graphs from the binary and answers with text, never with pixels: the parent only sees the
//! recognised lines. It is a profile of its own so an OCR model that fails, or a
//! picture that takes too long, cannot take document parsing or face detection
//! down with it.
//!
//! Request: `NFX1`, the operation byte, a little-endian `u32` option word, then
//! the image's bytes. Reply: the common frame, `T` for UTF-8 text, `U` for
//! "examined, no indexable text", `E` for "the environment or a model failed".

use std::io::Read;

use crate::ocr::{self, OcrOptions};
use crate::sandbox::worker::{self, RunOutcome};
use crate::sandbox::{Grants, JobVerdict, Profile};

/// Recognise text, which is this profile's only operation.
pub const OP_OCR: u8 = b'O';

/// Most image bytes the child will accept: the parent already caps what it reads
/// (32 MiB), and this is the second half of the same bound.
pub(crate) const MAX_IMAGE_BYTES: usize = crate::ocr::MAX_IMAGE_BYTES;

/// The gate that bounds how many OCR children are alive at once.
///
/// Separate from the structured and image gates: a slow OCR pass must not hold a
/// document permit, and a queue of images must not starve OCR. A constant, so it
/// cannot be set unlimited.
pub(crate) const MAX_CONCURRENT_OCR_WORKERS: usize = 2;

static OCR_CONCURRENCY: std::sync::OnceLock<std::sync::Arc<tokio::sync::Semaphore>> =
    std::sync::OnceLock::new();

/// Acquire a permit covering one OCR child; callers queue rather than fail.
pub(crate) async fn acquire_ocr_permit() -> Result<tokio::sync::OwnedSemaphorePermit, String> {
    OCR_CONCURRENCY
        .get_or_init(|| {
            std::sync::Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_OCR_WORKERS))
        })
        .clone()
        .acquire_owned()
        .await
        .map_err(|error| format!("OCR concurrency gate failed: {error}"))
}

/// Ask the confined child to read one image.
///
/// The reply distinguishes a verdict on the bytes (`U`, terminal) from an
/// environment/model failure (`E`, retryable).
pub(crate) fn extract(content: &[u8], options: OcrOptions) -> worker::Outcome {
    let request = request(OP_OCR, encode_options(options), content);
    match worker::run_request(
        Profile::Ocr,
        worker::OCR_TIMEOUT,
        request,
        Grants::default(),
    ) {
        RunOutcome::Reply { tag: b'T', body } => match String::from_utf8(body) {
            Ok(text) => worker::Outcome::Extracted(crate::indexer::extract::Extracted::Text(text)),
            Err(_) => worker::Outcome::Failed(crate::indexer::extract::reason::FAILED),
        },
        RunOutcome::Reply { tag: b'U', body } => {
            let reason = String::from_utf8_lossy(&body);
            worker::Outcome::Extracted(crate::indexer::extract::Extracted::Unsupported(
                crate::indexer::extract::reason::intern(&reason),
            ))
        }
        RunOutcome::Reply { tag: b'E', body } => {
            worker::Outcome::Unavailable(String::from_utf8_lossy(&body).into_owned())
        }
        RunOutcome::Reply { .. } => {
            worker::Outcome::Failed(crate::indexer::extract::reason::FAILED)
        }
        RunOutcome::Unavailable(why) => worker::Outcome::Unavailable(why),
        RunOutcome::Failed(why) => worker::Outcome::Failed(why),
    }
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
        return worker::write_frame(b'U', b"image over the size cap");
    }

    let (tag, body) = answer(op, decode_options(options), &data);
    worker::write_frame(tag, &body)
}

/// Do the work one operation names, as a reply frame's tag and body.
fn answer(op: u8, options: OcrOptions, data: &[u8]) -> (u8, Vec<u8>) {
    match op {
        OP_OCR => match read(data, options) {
            Ok(text) => (b'T', text.into_bytes()),
            Err(Refusal::NoText(reason)) => (b'U', reason.as_bytes().to_vec()),
            Err(Refusal::Unavailable(reason)) => (b'E', reason.into_bytes()),
        },
        _ => (b'U', b"unknown OCR operation".to_vec()),
    }
}

/// Why the child did not return text.
enum Refusal {
    /// A verdict on the bytes: terminal until the content or extractor changes.
    NoText(&'static str),
    /// The model could not load or run: an environment problem, retryable.
    Unavailable(String),
}

fn read(data: &[u8], options: OcrOptions) -> Result<String, Refusal> {
    let image = crate::thumbnail_util::load_image_with_orientation(data)
        .map_err(|_| Refusal::NoText(crate::indexer::extract::reason::NO_TEXT))?;
    let text = ocr::recognize(&image, options).map_err(Refusal::Unavailable)?;
    if text.trim().is_empty() {
        Err(Refusal::NoText(crate::indexer::extract::reason::NO_TEXT))
    } else {
        Ok(text)
    }
}

/// The wire format is the faces one: score in the high half, side in the low.
fn encode_options(options: OcrOptions) -> u32 {
    let options = options.normalized();
    let score = (options.min_score * 1000.0).round() as u32;
    (score << 16) | options.max_side
}

fn decode_options(raw: u32) -> OcrOptions {
    if raw == 0 {
        return OcrOptions::default();
    }
    OcrOptions {
        min_score: ((raw >> 16) as f32 / 1000.0).clamp(0.0, 1.0),
        max_side: (raw & 0xffff).clamp(320, 1600),
    }
}

/// The self-test: load all models and run one blank page and line orientation
/// classification, one detection and one recognition pass.
pub fn probe() -> (String, JobVerdict) {
    match ocr::engine::probe() {
        Ok(lines) => (format!("ocr-ok({lines})"), JobVerdict::Ok),
        Err(error) => (format!("ocr-failed({})", token(&error)), JobVerdict::Failed),
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
        let request = request(OP_OCR, encode_options(OcrOptions::default()), b"payload");
        assert_eq!(&request[..4], worker::MAGIC);
        assert_eq!(request[4], OP_OCR);
        assert_eq!(
            u32::from_le_bytes(request[5..9].try_into().unwrap()),
            encode_options(OcrOptions::default())
        );
        assert_eq!(&request[9..], b"payload");
    }

    #[test]
    fn the_options_survive_the_wire() {
        let options = OcrOptions {
            min_score: 0.42,
            max_side: 640,
        };
        let decoded = decode_options(encode_options(options));
        assert!((decoded.min_score - 0.42).abs() < 0.001, "{decoded:?}");
        assert_eq!(decoded.max_side, 640);
        // Zero is the "use the defaults" word, not a zero threshold.
        let defaults = decode_options(0);
        assert_eq!(defaults.min_score, OcrOptions::default().min_score);
        assert_eq!(defaults.max_side, OcrOptions::default().max_side);
    }

    #[test]
    fn undecodable_bytes_are_a_refusal() {
        let (tag, _) = answer(OP_OCR, OcrOptions::default(), b"not an image");
        assert_eq!(tag, b'U');
    }

    #[test]
    fn an_unknown_operation_is_refused() {
        let (tag, body) = answer(b'?', OcrOptions::default(), b"");
        assert_eq!(tag, b'U');
        assert_eq!(std::str::from_utf8(&body).unwrap(), "unknown OCR operation");
    }
}
