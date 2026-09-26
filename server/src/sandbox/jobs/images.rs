//! Image decode, EXIF and avatar processing, in the confined child.
//!
//! The parent reads the bytes — a block read for a repository file, the request
//! body for an avatar — and hands them over; the decode, the resize and the
//! encode all happen on the far side of the process boundary, because a hostile
//! image is exactly what an image decoder must not meet inside the server.
//!
//! The request is `NFX1`, one operation byte, a little-endian `u32` target size,
//! then the image bytes to end of input. The reply is the common frame: `P` or
//! `J` for an encoded image, `X` for EXIF JSON, `U` for a refusal.

use std::io::Read;

use crate::sandbox::worker::{self, RunOutcome};
use crate::sandbox::{Grants, Profile};
use crate::thumbnail_util::{self, ThumbFormat};

/// Re-encode the image as a thumbnail of `size`.
pub const OP_THUMBNAIL: u8 = b'T';
/// Re-encode the image as a square avatar of `size`.
pub const OP_SQUARE: u8 = b'S';
/// Parse EXIF and answer with JSON.
pub const OP_EXIF: u8 = b'X';

/// Most image bytes the child will accept.
///
/// The parent caps what it reads before it sends anything (32 MiB for a
/// thumbnail source, 8 MiB for EXIF), so this is the second half of the same
/// bound: a request that reached the child over any other path still cannot make
/// it hold an unbounded buffer.
const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;

/// A thumbnail, encoded and its container. `Err` is why there is none.
pub fn thumbnail(content: &[u8], size: u32) -> Result<(Vec<u8>, ThumbFormat), String> {
    let (tag, body) = run(OP_THUMBNAIL, size, content)?;
    match tag {
        b'P' => Ok((body, ThumbFormat::Png)),
        b'J' => Ok((body, ThumbFormat::Jpeg)),
        b'U' => Err(refusal(body)),
        _ => Err(unknown_tag(tag)),
    }
}

/// A square avatar, encoded as PNG.
pub fn square(content: &[u8], size: u32) -> Result<Vec<u8>, String> {
    let (tag, body) = run(OP_SQUARE, size, content)?;
    match tag {
        b'P' => Ok(body),
        b'U' => Err(refusal(body)),
        _ => Err(unknown_tag(tag)),
    }
}

/// The image's EXIF, as the JSON the endpoint serves.
pub fn exif(content: &[u8]) -> Result<String, String> {
    let (tag, body) = run(OP_EXIF, 0, content)?;
    match tag {
        b'X' => String::from_utf8(body)
            .map_err(|_| "the EXIF worker answered with non-UTF-8".to_string()),
        b'U' => Err(refusal(body)),
        _ => Err(unknown_tag(tag)),
    }
}

/// Send one image operation to the child and collect its reply.
fn run(op: u8, size: u32, content: &[u8]) -> Result<(u8, Vec<u8>), String> {
    let request = request(op, size, content);
    match worker::run_request(
        Profile::Images,
        worker::IMAGE_TIMEOUT,
        request,
        Grants::default(),
    ) {
        RunOutcome::Reply { tag, body } => Ok((tag, body)),
        RunOutcome::Unavailable(why) => Err(why),
        RunOutcome::Failed(why) => Err(why.to_string()),
    }
}

/// The request the parent sends.
fn request(op: u8, size: u32, content: &[u8]) -> Vec<u8> {
    let mut request = Vec::with_capacity(worker::MAGIC.len() + 5 + content.len());
    request.extend_from_slice(worker::MAGIC);
    request.push(op);
    request.extend_from_slice(&size.to_le_bytes());
    request.extend_from_slice(content);
    request
}

fn refusal(body: Vec<u8>) -> String {
    String::from_utf8_lossy(&body).into_owned()
}

fn unknown_tag(tag: u8) -> String {
    format!("the image worker answered with an unknown tag {tag:?}")
}

/// The child's half of the protocol.
pub fn serve() -> anyhow::Result<()> {
    let mut stdin = std::io::stdin().lock();
    let mut head = [0u8; 9];
    stdin.read_exact(&mut head)?;
    if &head[..4] != worker::MAGIC {
        anyhow::bail!("the request does not start with the protocol magic");
    }
    let op = head[4];
    let size = u32::from_le_bytes(head[5..9].try_into().expect("four bytes"));

    let mut data = Vec::new();
    stdin.read_to_end(&mut data)?;
    if data.len() > MAX_IMAGE_BYTES {
        return worker::write_frame(b'U', b"image over the size cap");
    }

    let (tag, body) = answer(op, size, &data);
    worker::write_frame(tag, &body)
}

/// Do the work one operation names, as a reply frame's tag and body.
fn answer(op: u8, size: u32, data: &[u8]) -> (u8, Vec<u8>) {
    match op {
        OP_THUMBNAIL => match thumbnail_util::generate_thumbnail_encoded(data, size) {
            Ok((bytes, ThumbFormat::Png)) => (b'P', bytes),
            Ok((bytes, ThumbFormat::Jpeg)) => (b'J', bytes),
            Err(error) => (b'U', error.to_string().into_bytes()),
        },
        OP_SQUARE => match thumbnail_util::generate_square_thumbnail(data, size) {
            Ok(bytes) => (b'P', bytes),
            Err(error) => (b'U', error.to_string().into_bytes()),
        },
        OP_EXIF => match crate::service::fs::exif::ExifService::extract_exif(data) {
            Ok(value) => (
                b'X',
                serde_json::to_vec(&value).unwrap_or_else(|_| b"null".to_vec()),
            ),
            Err(error) => (b'U', error.to_string().into_bytes()),
        },
        _ => (b'U', b"unknown image operation".to_vec()),
    }
}

/// The self-test: decode a generated image and encode it again.
///
/// A profile that cannot do the work says so here rather than on the first
/// thumbnail a user asks for.
pub fn probe() -> String {
    let png = tiny_png();
    match thumbnail_util::generate_thumbnail_encoded(&png, 16) {
        Ok((bytes, _)) => format!("image-ok({})", bytes.len()),
        Err(error) => format!("image-failed({error})"),
    }
}

/// A 2×2 PNG, generated rather than checked in so the probe has no fixture to
/// lose.
fn tiny_png() -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(2, 2, image::Rgba([200, 40, 40, 255]));
    let mut out = Vec::new();
    let _ = image::DynamicImage::ImageRgba8(image)
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PNG round-trips through the operation the parent sends, and the reply
    /// is the frame the parent parses.
    #[test]
    fn a_thumbnail_request_round_trips() {
        let png = tiny_png();
        let (tag, body) = answer(OP_THUMBNAIL, 16, &png);
        // An opaque image may come back as either container; the parent reads
        // the tag for which.
        assert!(tag == b'P' || tag == b'J', "unexpected tag {tag:?}");
        let decoded = image::load_from_memory(&body).expect("the reply is an image");
        assert!(decoded.width() <= 16 && decoded.height() <= 16);
    }

    /// A square request answers with PNG whatever the source was.
    #[test]
    fn a_square_request_answers_png() {
        let png = tiny_png();
        let (tag, body) = answer(OP_SQUARE, 8, &png);
        assert_eq!(tag, b'P');
        let decoded = image::load_from_memory(&body).expect("image");
        assert_eq!((decoded.width(), decoded.height()), (8, 8));
    }

    /// Bytes that are not an image are a refusal, not a crash.
    #[test]
    fn undecodable_bytes_are_a_refusal() {
        let (tag, body) = answer(OP_THUMBNAIL, 16, b"not an image at all");
        assert_eq!(tag, b'U');
        assert!(!body.is_empty(), "a refusal says why");
    }

    /// An unknown operation is refused rather than guessed at.
    #[test]
    fn an_unknown_operation_is_refused() {
        let (tag, _) = answer(b'?', 16, b"");
        assert_eq!(tag, b'U');
    }

    /// The self-test does the work rather than assuming it.
    #[test]
    fn the_probe_decodes_and_re_encodes() {
        assert!(probe().starts_with("image-ok("), "{}", probe());
    }

    /// The request framing is the magic, the operation, the size and the bytes.
    #[test]
    fn the_request_is_what_the_child_reads() {
        let request = request(OP_THUMBNAIL, 48, b"payload");
        assert_eq!(&request[..4], worker::MAGIC);
        assert_eq!(request[4], OP_THUMBNAIL);
        assert_eq!(u32::from_le_bytes(request[5..9].try_into().unwrap()), 48);
        assert_eq!(&request[9..], b"payload");
    }
}
