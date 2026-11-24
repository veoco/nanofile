//! Optical character recognition for the full-text index.
//!
//! PP-OCRv6 detection and recognition and the PP-LCNet page and text-line
//! orientation classifiers run in the confined `Profile::Ocr` child on the
//! fixed-model inference runtime. The parent only ever receives the recognised
//! text; images and pixel buffers stay on the far side of the process boundary.
//!
//! The pipeline and per-model details are in `docs/dev/ocr.md`; PP-OCRv6 is
//! Apache-2.0 and the ONNX files are embedded, not downloaded at runtime.

pub mod crop;
pub mod det;
pub mod engine;
pub mod keys;
pub mod layout;
pub mod line_orientation;
pub mod orientation;
pub mod pdf;
pub mod rec;
pub mod resample;

pub use engine::{
    DEFAULT_MAX_SIDE, DEFAULT_MIN_SCORE, MAX_IMAGE_BYTES, OcrOptions, Recognizer, Source, is_image,
    recognize, source_for,
};

/// Whether the path names a PDF, the other format OCR can read page by page.
///
/// The check matches [`crate::indexer::extract::plan`]: extension only, ASCII
/// case-insensitive.
pub fn is_pdf(path: &str) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("pdf"))
}

#[cfg(test)]
mod tests {
    use super::is_pdf;

    #[test]
    fn pdf_extensions_are_case_insensitive() {
        assert!(is_pdf("scan.pdf"));
        assert!(is_pdf("SCAN.PDF"));
        assert!(!is_pdf("scan.txt"));
        assert!(!is_pdf("pdf"));
    }
}
