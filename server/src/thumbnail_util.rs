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

// ─── Public API ───────────────────────────────────────────────────────────

/// Decode image bytes, apply EXIF orientation, then produce a **square**
/// thumbnail (center-crop + resize-exact).  Used for **avatar** thumbnails,
/// matching seahub's `AvatarBase.create_thumbnail()` behaviour.
pub fn generate_square_thumbnail(content: &[u8], size: u32) -> Result<Vec<u8>, AppError> {
    let img = load_image_with_orientation(content, size)?;
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
    let img = load_image_with_orientation(content, size)?;
    let thumb = img.resize(size, size, FilterType::Triangle);
    encode_png(&thumb)
}

/// Check whether a file extension corresponds to a supported thumbnail format.
///
/// This should stay in sync with the set of image formats enabled by the
/// `image` crate via `default-formats` + `avif-native`.
pub fn is_supported_image_ext(ext: &str) -> bool {
    matches!(
        ext,
        "avif"
            | "bmp"
            | "dds"
            | "exr"
            | "fif"
            | "gif"
            | "hdr"
            | "ico"
            | "jpg"
            | "jpeg"
            | "png"
            | "pnm"
            | "ppm"
            | "pgm"
            | "pbm"
            | "qoi"
            | "tga"
            | "tif"
            | "tiff"
            | "webp"
    )
}

// ─── Internal helpers ─────────────────────────────────────────────────────

/// Decode raw image bytes and apply any EXIF orientation tag.
fn load_image_with_orientation(bytes: &[u8], _target_size: u32) -> Result<DynamicImage, AppError> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| AppError::Internal(format!("image format detection failed: {e}")))?;

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
