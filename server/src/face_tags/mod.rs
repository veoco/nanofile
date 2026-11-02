//! Automatic "people" tags from YuNet face detection.
//!
//! This module is deliberately small: it detects faces and counts them. It does
//! not produce embeddings, face crops or identities, so no biometric template is
//! ever stored. The count is turned into `人物`/`单人`/`合影` tags by the service
//! layer.
//!
//! Everything here runs inside the confined image worker; the parent only ever
//! sees the JSON summary (count + best score).

pub mod engine;
pub mod labels;
pub mod preprocess;
pub mod yunet;

/// Bump when the model, preprocessing or the counting rules change. Backfill
/// compares this against `face_tag_scans.model_version`.
pub const MODEL_VERSION: u32 = 1;

/// Detection knobs shared by the worker and tests.
#[derive(Clone, Copy, Debug)]
pub struct DetectOptions {
    /// Minimum `sqrt(cls * obj)` score for a face to be counted.
    pub min_score: f32,
    /// Minimum short edge, in original-image pixels, for a face to be counted.
    pub min_face_size: f32,
}

impl Default for DetectOptions {
    fn default() -> Self {
        Self {
            min_score: 0.7,
            min_face_size: 32.0,
        }
    }
}
