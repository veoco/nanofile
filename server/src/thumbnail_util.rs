//! Shared thumbnail-generation utilities.
//!
//! Consolidates three identical copies of `generate_thumbnail` that existed in
//! `fs/service/thumbnail.rs`, `user/service/avatar.rs`, and `ui/settings.rs`.
//! Applies EXIF orientation on decode, and provides a square-crop variant for
//! avatar thumbnails.

use std::io::Cursor;

use image::DynamicImage;
use image::ImageDecoder;
use image::ImageReader;
use image::imageops::FilterType;

use base::error::AppError;

/// Upper bound for thumbnail/avatar **output** dimensions. Resizing to
/// `size × size` at unbounded sizes would allocate `size² × 4` bytes (a 100000px
/// thumb needs ~100 GB) and abort the process. Real clients use sizes ≤ 256.
pub const MAX_THUMBNAIL_SIZE: u32 = 1024;

/// Upper bound on decoded **source** dimensions. 8192 covers 8K photos
/// (7680×4320); anything larger is a decompression-bomb attempt.
const MAX_SOURCE_DIMENSION: u32 = 8192;

// ─── Public API ───────────────────────────────────────────────────────────

/// Decode image bytes, apply EXIF orientation, then produce a **square**
/// thumbnail (center-crop + resize-exact).  Used for **avatar** thumbnails,
/// matching seahub's `AvatarBase.create_thumbnail()` behaviour.
pub fn generate_square_thumbnail(content: &[u8], size: u32) -> Result<Vec<u8>, AppError> {
    let size = size.min(MAX_THUMBNAIL_SIZE);
    let img = load_image_with_orientation(content)?;
    let (w, h) = (img.width(), img.height());
    let side = w.min(h);
    let x = (w - side) / 2;
    let y = (h - side) / 2;
    let cropped = img.crop_imm(x, y, side, side);
    let resized = image::imageops::resize(&cropped, size, size, FilterType::Lanczos3);
    encode_png(&DynamicImage::from(resized))
}

/// Decode image bytes, apply EXIF orientation, then produce a **same-ratio**
/// thumbnail (fits inside `size × size`).  Used for **file** thumbnails,
/// matching seahub's `_create_thumbnail_common()` behaviour.
///
/// Uses `Triangle` filter (faster than Lanczos3) — quality difference at
/// thumbnail sizes is imperceptible.
pub fn generate_thumbnail(content: &[u8], size: u32) -> Result<Vec<u8>, AppError> {
    let size = size.min(MAX_THUMBNAIL_SIZE);
    let img = load_image_with_orientation(content)?;
    let thumb = img.resize(size, size, FilterType::Triangle);
    encode_png(&thumb)
}

/// Check whether a file extension corresponds to a supported thumbnail format.
pub fn is_supported_image_ext(ext: &str) -> bool {
    matches!(
        ext,
        "bmp" | "gif" | "ico" | "jpg" | "jpeg" | "png" | "webp" | "tiff" | "tif"
    )
}

/// Whether an extension is a video file that can be thumbnailed via ffmpeg.
/// Single source of truth — the UI's `is_video_file` delegates to this.
pub fn is_video_ext(ext: &str) -> bool {
    matches!(
        ext,
        "mp4" | "mov" | "avi" | "mkv" | "webm" | "wmv" | "flv" | "3gp"
    )
}

/// Whether an extension is an audio file whose embedded cover art can be
/// extracted as a thumbnail via ffmpeg. Single source of truth — the UI's
/// `is_audio_file` delegates to this.
pub fn is_audio_ext(ext: &str) -> bool {
    matches!(
        ext,
        "mp3" | "flac" | "wav" | "ogg" | "m4a" | "aac" | "wma" | "opus"
    )
}

/// Whether an extension is a still image that the in-process image crate
/// cannot decode but ffmpeg can (HEIC/HEIF photos, AVIF web images). These
/// take the ffmpeg thumbnail path like video/audio. Single source of truth.
pub fn is_ffmpeg_image_ext(ext: &str) -> bool {
    matches!(ext, "heic" | "heif" | "avif")
}

/// Whether an extension can produce a thumbnail via either the in-process
/// image decoder or ffmpeg. Used by the UI to decide whether to advertise a
/// thumbnail URL for a file.
pub fn is_thumbnail_image_ext(ext: &str) -> bool {
    is_supported_image_ext(ext) || is_ffmpeg_image_ext(ext)
}

// ─── Internal helpers ─────────────────────────────────────────────────────

/// Decode raw image bytes and apply any EXIF orientation tag.
fn load_image_with_orientation(bytes: &[u8]) -> Result<DynamicImage, AppError> {
    // Cap the decoded source dimensions so a small file that declares a huge
    // size (a decompression bomb) is rejected before the decoder allocates
    // width×height×bytes, which would abort the process. `max_alloc` keeps the
    // crate default (512 MiB).
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_DIMENSION);
    limits.max_image_height = Some(MAX_SOURCE_DIMENSION);
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| AppError::Internal(format!("image format detection failed: {e}")))?;
    // into_decoder applies these limits to the decoder and propagates a
    // dimension-over-limit error before the full decode allocates.
    reader.limits(limits);

    let mut decoder = reader
        .into_decoder()
        .map_err(|e| AppError::Internal(format!("image decoder creation failed: {e}")))?;

    // Read orientation from the decoder's EXIF metadata (JPEG/WebP/PNG supported)
    let orientation = decoder.orientation().ok();

    let mut img = DynamicImage::from_decoder(decoder)
        .map_err(|e| AppError::Internal(format!("image decode failed: {e}")))?;

    if let Some(orient) = orientation
        && orient != image::metadata::Orientation::NoTransforms
    {
        img.apply_orientation(orient);
    }

    Ok(img)
}

/// Encode a `DynamicImage` as PNG bytes.
fn encode_png(img: &DynamicImage) -> Result<Vec<u8>, AppError> {
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        .map_err(|e| AppError::Internal(format!("PNG encode failed: {e}")))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_png_bytes(img: image::RgbaImage) -> Vec<u8> {
        encode_png(&image::DynamicImage::ImageRgba8(img)).expect("PNG encode failed")
    }

    /// A tiny JPEG whose SOF0 declares 65535×65535 is rejected without
    /// panicking or allocating a multi-GB buffer. (The exact dimension-limit
    /// boundary is exercised by `png_dimension_limit_boundary`; here a
    /// truncated bomb must simply fail closed.)
    #[test]
    fn jpeg_declaring_huge_dimensions_is_rejected() {
        // SOI | SOF0 (len 11, prec 8, h=65535, w=65535, 1 component) | EOI
        let bomb = [
            0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 0x08, 0xFF, 0xFF, 0xFF, 0xFF, 0x01, 0x11, 0x01,
            0x00, 0xFF, 0xD9,
        ];
        assert!(generate_thumbnail(&bomb, 48).is_err());
        assert!(generate_square_thumbnail(&bomb, 48).is_err());
    }

    /// A real PNG just over the dimension limit is rejected; at the limit it
    /// decodes fine (boundary check).
    #[test]
    fn png_dimension_limit_boundary() {
        let over = encode_png_bytes(image::RgbaImage::new(8193, 16));
        assert!(
            generate_thumbnail(&over, 48).is_err(),
            "8193-wide PNG must be rejected"
        );

        let at_limit = encode_png_bytes(image::RgbaImage::new(8192, 16));
        let thumb = generate_thumbnail(&at_limit, 48)
            .expect("8192-wide PNG should decode within the limit");
        assert!(!thumb.is_empty());
    }

    /// A normal image still produces a thumbnail.
    #[test]
    fn normal_image_thumbnail_ok() {
        let mut img = image::RgbaImage::new(16, 16);
        for p in img.pixels_mut() {
            *p = image::Rgba([10u8, 20, 30, 255]);
        }
        let thumb =
            generate_thumbnail(&encode_png_bytes(img), 16).expect("small PNG should decode");
        assert!(!thumb.is_empty());
    }
}
