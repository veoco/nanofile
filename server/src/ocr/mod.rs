//! Optical character recognition for the full-text index.
//!
//! PP-OCRv6 tiny detection and recognition, and the PP-LCNet page and text-line
//! orientation classifiers, run in the confined `Profile::Ocr` child through
//! tract. The parent only ever receives the recognised text; images, pixel
//! buffers and model plans stay on the far side of the process boundary.
//!
//! This module owns the whole pipeline in Rust:
//!
//! * [`orientation`] classifies the page as 0/90/180/270 degrees and turns it
//!   upright before detection, so a sideways scan is still read;
//! * [`line_orientation`] flips a detected line that is still upside down
//!   inside an otherwise upright page;
//! * [`det`] resizes/normalises the image, runs the DB detection graph and turns
//!   its probability map into text boxes (connected components, convex hull,
//!   minimum-area rectangle and the DB unclip expansion);
//! * [`crop`] perspective-warps each box out of the source image;
//! * [`rec`] normalises a line, runs the CTC recognition graph and decodes it
//!   against the embedded character dictionary in [`keys`];
//! * [`layout`] orders the lines into the text the index stores.
//!
//! `PP-OCRv6` is Apache-2.0; the DB post-processing follows the published
//! PaddleOCR algorithm. The ONNX files are embedded, not downloaded at runtime.

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
