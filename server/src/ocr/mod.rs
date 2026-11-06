//! Optical character recognition for the full-text index.
//!
//! PP-OCRv6 tiny detection and recognition, and the PP-LCNet page orientation
//! classifier, run in the confined `Profile::Ocr` child through tract. The
//! parent only ever receives the recognised text; images, pixel buffers and
//! model plans stay on the far side of the process boundary.
//!
//! This module owns the whole pipeline in Rust:
//!
//! * [`orientation`] classifies the page as 0/90/180/270 degrees and turns it
//!   upright before detection, so a sideways scan is still read;
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
pub mod orientation;
pub mod rec;
pub mod resample;

pub use engine::{
    DEFAULT_MAX_SIDE, DEFAULT_MIN_SCORE, MAX_IMAGE_BYTES, OcrOptions, Source, is_image, recognize,
    source_for,
};
