//! The OCR pipeline for one image.
//!
//! The model plans are prepared per concrete input size: detection varies with
//! the picture and each text line has its own width. The recognition widths are
//! bucketed by `rec::BUCKETS`, so one child prepares at most five rec plans.

use std::collections::HashMap;

use image::DynamicImage;

use crate::inference;

use super::crop::{self, Crop};
use super::det::{self, Geometry, Postprocess, TextBox};
use super::keys;
use super::layout;
use super::rec;

/// Default recognition confidence a line must reach to be indexed.
pub const DEFAULT_MIN_SCORE: f32 = 0.55;
/// Default longest edge of the detection input.
pub const DEFAULT_MAX_SIDE: u32 = 960;
/// Most image bytes the worker accepts; the parent reads at most this much.
pub const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;
/// Most text lines recognised per image.
const MAX_LINES: usize = 300;
/// Most text stored in the index, shared with the document extractor.
const MAX_TEXT_BYTES: usize = crate::indexer::extract::MAX_INDEXED_CONTENT_BYTES;

const DETECTION: &[u8] = include_bytes!("../../models/ppocr_v6_tiny_det.onnx");
const RECOGNITION: &[u8] = include_bytes!("../../models/ppocr_v6_tiny_rec.onnx");

/// Where the bytes of an image can be decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The `image` crate decodes it, in the OCR child.
    InProcess,
    /// The `image` crate cannot; the media helper decodes a still first.
    Media,
}

/// Detection/recognition knobs from `[ocr]`.
#[derive(Clone, Copy, Debug)]
pub struct OcrOptions {
    pub min_score: f32,
    pub max_side: u32,
}

impl Default for OcrOptions {
    fn default() -> Self {
        Self {
            min_score: DEFAULT_MIN_SCORE,
            max_side: DEFAULT_MAX_SIDE,
        }
    }
}

impl OcrOptions {
    /// Clamp values an operator may have written directly into the config file.
    pub fn normalized(self) -> Self {
        Self {
            min_score: self.min_score.clamp(0.0, 1.0),
            max_side: self.max_side.clamp(320, 1600),
        }
    }
}

/// Whether this path names an image OCR can read, and how.
pub fn source_for(filename: &str) -> Option<Source> {
    let extension = filename
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    if crate::thumbnail_util::is_supported_image_ext(&extension) {
        Some(Source::InProcess)
    } else if crate::thumbnail_util::is_ffmpeg_image_ext(&extension) {
        Some(Source::Media)
    } else {
        None
    }
}

/// Whether the path names an image OCR can read.
pub fn is_image(path: &str) -> bool {
    source_for(path).is_some()
}

struct Engine {
    detection: inference::OnnxModel,
    recognition: inference::OnnxModel,
    detection_plans: HashMap<(usize, usize), tract::Runnable>,
    recognition_plans: HashMap<usize, tract::Runnable>,
}

impl Engine {
    fn new() -> Result<Self, String> {
        Ok(Self {
            detection: inference::OnnxModel::load(DETECTION)?,
            recognition: inference::OnnxModel::load(RECOGNITION)?,
            detection_plans: HashMap::new(),
            recognition_plans: HashMap::new(),
        })
    }

    fn detect(&mut self, tensor: &[f32], geometry: Geometry) -> Result<Vec<TextBox>, String> {
        let key = (geometry.height, geometry.width);
        if !self.detection_plans.contains_key(&key) {
            let spec = format!("1,3,{},{},f32", geometry.height, geometry.width);
            let plan = self.detection.plan(&spec)?;
            self.detection_plans.insert(key, plan);
        }
        let plan = self
            .detection_plans
            .get(&key)
            .ok_or_else(|| "the detection plan disappeared".to_string())?;
        let outputs = inference::run(plan, &[1, 3, geometry.height, geometry.width], tensor)?;
        if outputs.len() != 1 {
            return Err(format!(
                "the OCR detection graph returned {} outputs, expected 1",
                outputs.len()
            ));
        }
        let shape = inference::shape(&outputs[0])?;
        if shape != vec![1, 1, geometry.height, geometry.width] {
            return Err(format!(
                "the OCR detection output has shape {shape:?}, expected [1, 1, {}, {}]",
                geometry.height, geometry.width
            ));
        }
        let probability = inference::values(&outputs[0])?;
        Ok(det::postprocess(
            probability,
            geometry.width,
            geometry.height,
            geometry,
            Postprocess::default(),
        ))
    }

    fn recognize(&mut self, crop: &Crop, min_score: f32) -> Result<Option<rec::Decoded>, String> {
        let bucket = rec::bucket_for(rec::target_width(crop));
        if !self.recognition_plans.contains_key(&bucket) {
            let spec = format!("1,3,48,{bucket},f32");
            let plan = self.recognition.plan(&spec)?;
            self.recognition_plans.insert(bucket, plan);
        }
        let plan = self
            .recognition_plans
            .get(&bucket)
            .ok_or_else(|| "the recognition plan disappeared".to_string())?;
        let tensor = rec::tensor(crop, bucket);
        let outputs = inference::run(plan, &[1, 3, rec::HEIGHT, bucket], &tensor)?;
        if outputs.len() != 1 {
            return Err(format!(
                "the OCR recognition graph returned {} outputs, expected 1",
                outputs.len()
            ));
        }
        let shape = inference::shape(&outputs[0])?;
        if shape.len() != 3 || shape[0] != 1 || shape[2] != keys::CLASSES {
            return Err(format!(
                "the OCR recognition output has shape {shape:?}, expected [1, T, {}]",
                keys::CLASSES
            ));
        }
        let logits = inference::values(&outputs[0])?;
        Ok(rec::decode(logits, shape[1], shape[2], min_score))
    }
}

/// Run OCR over a decoded image and return the text the index stores.
///
/// An image with no text is an empty string, not an error: the caller records
/// that as "OCR ran and found nothing".
pub fn recognize(image: &DynamicImage, options: OcrOptions) -> Result<String, String> {
    let rgb = image.to_rgb8();
    let width = rgb.width() as usize;
    let height = rgb.height() as usize;
    if width == 0 || height == 0 {
        return Ok(String::new());
    }
    let options = options.normalized();
    let geometry = Geometry::new(width, height, options.max_side);

    let mut engine = Engine::new()?;
    let tensor = det::tensor(rgb.as_raw(), width, height, geometry);
    let boxes = engine.detect(&tensor, geometry)?;

    let mut lines = Vec::new();
    for text_box in boxes.into_iter().take(MAX_LINES) {
        let Some(crop) = crop::warp(rgb.as_raw(), width, height, text_box.quad) else {
            continue;
        };
        if let Some(decoded) = engine.recognize(&crop, options.min_score)? {
            lines.push(layout::Line {
                text: decoded.text,
                quad: text_box.quad,
            });
        }
    }

    let mut text = layout::join(lines);
    if text.len() > MAX_TEXT_BYTES {
        let cut = floor_char_boundary(&text, MAX_TEXT_BYTES);
        text.truncate(cut);
    }
    Ok(text)
}

/// Load both graphs and run one blank detection and recognition.
///
/// This is the startup probe of the OCR profile: it proves the confined child
/// can prepare and run every model it needs, without depending on a fixture.
pub fn probe() -> Result<usize, String> {
    let mut engine = Engine::new()?;
    let geometry = Geometry {
        width: 64,
        height: 64,
        ratio_w: 1.0,
        ratio_h: 1.0,
    };
    let detection = vec![0f32; 3 * 64 * 64];
    if !engine.detect(&detection, geometry)?.is_empty() {
        return Err("the detector found text on a blank probe".to_string());
    }
    let crop = Crop {
        rgb: vec![0u8; 160 * 48 * 3],
        width: 160,
        height: 48,
    };
    if engine.recognize(&crop, 0.0)?.is_some() {
        return Err("the recognizer read a blank probe".to_string());
    }
    Ok(0)
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    if index >= text.len() {
        return text.len();
    }
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_extensions_choose_their_decoder() {
        assert_eq!(source_for("a.png"), Some(Source::InProcess));
        assert_eq!(source_for("a.JPG"), Some(Source::InProcess));
        assert_eq!(source_for("a.heic"), Some(Source::Media));
        assert_eq!(source_for("a.avif"), Some(Source::Media));
        assert_eq!(source_for("a.txt"), None);
        assert_eq!(source_for("no-extension"), None);
    }

    #[test]
    fn options_are_clamped() {
        let options = OcrOptions {
            min_score: 2.0,
            max_side: 10,
        }
        .normalized();
        assert_eq!(options.min_score, 1.0);
        assert_eq!(options.max_side, 320);
    }

    #[test]
    fn the_sample_page_is_read() {
        let bytes = include_bytes!("../../tests/fixtures/ocr/sample.png");
        let image = image::load_from_memory(bytes).expect("fixture");
        let text = recognize(&image, OcrOptions::default()).expect("recognize");
        assert!(
            text.contains("Markov Entropy Decomposition"),
            "the sample page was not read: {}",
            text.chars().take(500).collect::<String>()
        );
        assert!(
            text.lines().count() >= 20,
            "the sample page produced {} lines",
            text.lines().count()
        );
    }
}
