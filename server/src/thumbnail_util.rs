//! Shared thumbnail-generation utilities.
//!
//! Consolidates three identical copies of `generate_thumbnail` that existed in
//! `fs/service/thumbnail.rs`, `user/service/avatar.rs`, and `ui/settings.rs`.
//! Applies EXIF orientation on decode, and provides a square-crop variant for
//! avatar thumbnails.
//!
//! File thumbnails are encoded by `generate_thumbnail_encoded`, which picks the
//! container from the pixels: PNG only when transparency has to survive, JPEG
//! otherwise. PNG is a poor fit for photographic content (a 640px PNG
//! thumbnail measured 0.5 MB against 70 KB for the same image as JPEG), and the
//! grid/gallery tiles are large enough that the old 256px PNG mosaic was both
//! blurry on a 2x display *and* heavier than the JPEG replacement.

use std::io::Cursor;

use image::DynamicImage;
use image::ImageDecoder;
use image::ImageReader;
use image::imageops::FilterType;

use base::error::AppError;

/// Upper bound for thumbnail/avatar **output** dimensions. Resizing to
/// `size × size` at unbounded sizes would allocate `size² × 4` bytes (a 100000px
/// thumb needs ~100 GB) and abort the process. Real clients use sizes ≤ 640.
pub const MAX_THUMBNAIL_SIZE: u32 = 1024;

/// Upper bound on decoded **source** dimensions. 8192 covers 8K photos
/// (7680×4320); anything larger is a decompression-bomb attempt.
const MAX_SOURCE_DIMENSION: u32 = 8192;

/// The long-edge sizes the web UI requests, and therefore the only sizes whose
/// cache entries are still reachable:
///
/// - `THUMBNAIL_SIZE_SMALL` backs the list-row icon.
/// - `THUMBNAIL_SIZE_LARGE` backs the grid tile (227 CSS px wide, i.e. 454
///   device px at 2x), the square gallery tile (369 device px on its short
///   edge) and the details-drawer preview. One size for all three keeps them a
///   single cache entry per file — switching between grid and gallery was
///   measured to hit the browser cache instead of the network — and 640 covers
///   every aspect ratio up to ~1.85:1 with no upscaling at 2x.
///
/// Raising the large size was only affordable because thumbnails became JPEG
/// unless the image actually uses transparency: the same photo measured 0.5 MB
/// at 640 as PNG against 70 KB as JPEG.
///
/// A cache entry at any other size is a leftover from an older UI — the
/// pre-640 browser asked for 256, which is why `RETAINED_THUMBNAIL_SIZES`
/// exists: the one-shot cache purge drops those files at startup, and the
/// `purge_legacy_thumbnail_sizes` migration drops their rows. The two lists are
/// deliberately kept in step by hand: a migration must not change behaviour
/// when these constants do.
pub const THUMBNAIL_SIZE_SMALL: u32 = 48;
pub const THUMBNAIL_SIZE_LARGE: u32 = 640;
pub const RETAINED_THUMBNAIL_SIZES: [u32; 2] = [THUMBNAIL_SIZE_SMALL, THUMBNAIL_SIZE_LARGE];

/// JPEG quality used for file thumbnails.
///
/// Measured on 1280px photographs fitted to 640px: q80/82/85/90 produce
/// 65/70/77/95 KB at 34.7/35.3/36.3/38.9 dB PSNR. 82 is the knee — q90 spends
/// 36% more bytes on a difference the browser's own downscale to the tile
/// averages away.
pub const JPEG_QUALITY: u8 = 82;

// ─── Public API ───────────────────────────────────────────────────────────

/// Container a thumbnail was encoded in.
///
/// Chosen per image by [`encode_thumbnail`] and persisted with the cached bytes
/// (`thumbnails.format`), so serving a cache hit never has to decode to learn
/// the content type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThumbFormat {
    /// Transparency has to survive: JPEG has no alpha channel, and the tile
    /// background differs between the light and dark themes, so there is no
    /// correct colour to composite onto.
    Png,
    /// Opaque pixels. Roughly 7x smaller than PNG for a photograph.
    Jpeg,
}

impl ThumbFormat {
    pub const fn mime(self) -> &'static str {
        match self {
            ThumbFormat::Png => "image/png",
            ThumbFormat::Jpeg => "image/jpeg",
        }
    }

    /// File extension for the on-disk cache entry.
    pub const fn ext(self) -> &'static str {
        match self {
            ThumbFormat::Png => "png",
            ThumbFormat::Jpeg => "jpg",
        }
    }

    /// Stable identifier stored in the database.
    pub const fn as_db(self) -> &'static str {
        match self {
            ThumbFormat::Png => "png",
            ThumbFormat::Jpeg => "jpeg",
        }
    }

    /// Decode a stored identifier. Anything unrecognised is read as PNG: rows
    /// written before the `format` column existed were all PNG, and a value
    /// from a future encoding must not be served as JPEG by an older binary.
    pub fn from_db(value: &str) -> Self {
        if value.eq_ignore_ascii_case("jpeg") {
            ThumbFormat::Jpeg
        } else {
            ThumbFormat::Png
        }
    }
}

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
/// thumbnail (fits inside `size × size`), encoded as PNG.
///
/// PNG-only: used by callers that need a container known up front (the avatar
/// sizes and the settings preview). File thumbnails go through
/// [`generate_thumbnail_encoded`] instead, which picks the container from the
/// pixels.
///
/// Uses `Triangle` filter (faster than Lanczos3) — quality difference at
/// thumbnail sizes is imperceptible.
pub fn generate_thumbnail(content: &[u8], size: u32) -> Result<Vec<u8>, AppError> {
    let size = size.min(MAX_THUMBNAIL_SIZE);
    let img = load_image_with_orientation(content)?;
    let thumb = img.resize(size, size, FilterType::Triangle);
    encode_png(&thumb)
}

/// Decode image bytes, apply EXIF orientation, fit inside `size × size`, and
/// encode in the container the pixels call for (see [`encode_thumbnail`]).
pub fn generate_thumbnail_encoded(
    content: &[u8],
    size: u32,
) -> Result<(Vec<u8>, ThumbFormat), AppError> {
    let size = size.min(MAX_THUMBNAIL_SIZE);
    let img = load_image_with_orientation(content)?;
    let thumb = img.resize(size, size, FilterType::Triangle);
    encode_thumbnail(&thumb)
}

/// Encode an already-fitted image: JPEG when it is opaque, PNG when any pixel
/// actually carries transparency.
pub fn encode_thumbnail(thumb: &DynamicImage) -> Result<(Vec<u8>, ThumbFormat), AppError> {
    if has_transparency(thumb) {
        Ok((encode_png(thumb)?, ThumbFormat::Png))
    } else {
        Ok((encode_jpeg(thumb, JPEG_QUALITY)?, ThumbFormat::Jpeg))
    }
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

/// Encode a `DynamicImage` as JPEG bytes at `quality`.
fn encode_jpeg(img: &DynamicImage, quality: u8) -> Result<Vec<u8>, AppError> {
    let mut out = Vec::new();
    // `to_rgb8` drops the alpha channel, which is only reachable for images
    // with no transparency at all (see `has_transparency`), so nothing is ever
    // composited onto an arbitrary background colour.
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut Cursor::new(&mut out), quality)
        .encode_image(&img.to_rgb8())
        .map_err(|e| AppError::Internal(format!("JPEG encode failed: {e}")))?;
    Ok(out)
}

/// Whether any pixel of `img` actually carries transparency.
///
/// The colour type is checked first, which is free: a JPEG, or a PNG the
/// decoder handed back as `Rgb8`/`Luma8`, can never be transparent. Only an
/// image that declares an alpha channel pays for the pixel scan — and that scan
/// covers the already-fitted thumbnail (at most `MAX_THUMBNAIL_SIZE²` pixels),
/// never the full-size source.
///
/// An image that declares alpha but never uses it (an RGBA PNG exported opaque,
/// which is common) still takes the JPEG path.
fn has_transparency(img: &DynamicImage) -> bool {
    if !img.color().has_alpha() {
        return false;
    }
    img.to_rgba8().pixels().any(|p| p.0[3] < u8::MAX)
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

    fn png_of(pixels: image::RgbaImage) -> Vec<u8> {
        encode_png_bytes(pixels)
    }

    /// The container follows the pixels: opaque content (including an image
    /// that merely *declares* an alpha channel) is JPEG, real transparency is
    /// PNG. The signature bytes are checked too, so the label cannot drift from
    /// what was actually written.
    #[test]
    fn encoding_choice_follows_transparency() {
        let opaque_alpha = png_of(image::RgbaImage::from_pixel(
            16,
            16,
            image::Rgba([10, 20, 30, 255]),
        ));
        let (data, format) =
            generate_thumbnail_encoded(&opaque_alpha, 16).expect("opaque RGBA should encode");
        assert_eq!(format, ThumbFormat::Jpeg, "opaque pixels must take JPEG");
        assert_eq!(&data[..2], &[0xFF, 0xD8], "not a JPEG (missing SOI)");

        // `RgbaImage::new` is fully transparent (alpha 0 everywhere).
        let transparent = png_of(image::RgbaImage::new(16, 16));
        let (data, format) =
            generate_thumbnail_encoded(&transparent, 16).expect("transparent PNG should encode");
        assert_eq!(format, ThumbFormat::Png, "transparency must survive as PNG");
        assert_eq!(
            &data[..8],
            b"\x89PNG\r\n\x1a\n",
            "not a PNG (missing signature)"
        );
    }

    /// A source with no alpha channel at all never reaches the pixel scan.
    #[test]
    fn rgb_source_is_jpeg() {
        let rgb = image::RgbImage::from_pixel(16, 16, image::Rgb([200, 100, 50]));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(rgb)
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .expect("PNG encode failed");

        let (_data, format) =
            generate_thumbnail_encoded(&bytes, 16).expect("RGB PNG should encode");
        assert_eq!(format, ThumbFormat::Jpeg);
    }

    /// The database identifier round-trips, and an unknown value (a row written
    /// before the column existed) reads back as PNG.
    #[test]
    fn format_db_identifier_round_trips() {
        for format in [ThumbFormat::Png, ThumbFormat::Jpeg] {
            assert_eq!(ThumbFormat::from_db(format.as_db()), format);
        }
        assert_eq!(ThumbFormat::from_db("JPEG"), ThumbFormat::Jpeg);
        assert_eq!(ThumbFormat::from_db("png"), ThumbFormat::Png);
        assert_eq!(ThumbFormat::from_db("webp"), ThumbFormat::Png);
        assert_eq!(ThumbFormat::Jpeg.mime(), "image/jpeg");
        assert_eq!(ThumbFormat::Png.ext(), "png");
    }
}
