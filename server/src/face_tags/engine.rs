//! Single-image entry point used by the confined image worker.
//!
//! Large photos are split into overlapping tiles so a face that is small in a
//! 20 MP group photo is not lost when the whole image is scaled down to YuNet's
//! 640 px input.

use image::DynamicImage;

use super::DetectOptions;
use super::preprocess::prepare;
use super::yunet::{Face, YuNet, nms};

/// Long edge above which the detector tiles instead of scaling the whole image.
const SINGLE_PASS_MAX_EDGE: u32 = 1600;
/// Upper bound on tiles per axis. Three covers a 6000 px image at 2000 px/tile.
const MAX_TILES: u32 = 3;
/// Overlap between tiles, as a fraction of a tile's size.
const TILE_OVERLAP: f32 = 0.1;

#[derive(Clone, Copy, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct DetectionSummary {
    pub face_count: usize,
    pub max_score: f32,
}

/// Detect faces and return the count of faces that meet `options`.
///
/// The count is what becomes the `人物`/`单人`/`合影` tags. Nothing from this
/// function is persisted except the count and the best score.
pub fn detect(image: &DynamicImage, options: DetectOptions) -> Result<DetectionSummary, String> {
    let model = YuNet::load()?;
    let (width, height) = (image.width(), image.height());
    if width == 0 || height == 0 {
        return Ok(DetectionSummary::default());
    }

    let grid = width
        .max(height)
        .div_ceil(SINGLE_PASS_MAX_EDGE)
        .clamp(1, MAX_TILES);
    let mut faces = Vec::new();

    if grid == 1 {
        let prepared = prepare(image)?;
        let detected = model.forward(&prepared)?;
        for face in detected {
            faces.push(map_face(face, &prepared, 0, 0));
        }
    } else {
        let base_w = width.div_ceil(grid);
        let base_h = height.div_ceil(grid);
        let overlap_w = (base_w as f32 * TILE_OVERLAP).round() as u32;
        let overlap_h = (base_h as f32 * TILE_OVERLAP).round() as u32;
        for gy in 0..grid {
            for gx in 0..grid {
                let x0 = gx.saturating_mul(base_w).saturating_sub(overlap_w);
                let y0 = gy.saturating_mul(base_h).saturating_sub(overlap_h);
                let x1 = ((gx + 1).saturating_mul(base_w) + overlap_w).min(width);
                let y1 = ((gy + 1).saturating_mul(base_h) + overlap_h).min(height);
                if x1 <= x0 || y1 <= y0 {
                    continue;
                }
                let tile = image.crop_imm(x0, y0, x1 - x0, y1 - y0);
                let prepared = prepare(&tile)?;
                let detected = model.forward(&prepared)?;
                for face in detected {
                    faces.push(map_face(face, &prepared, x0, y0));
                }
            }
        }
    }

    nms(&mut faces, 0.3, 100);
    let mut summary = DetectionSummary::default();
    for face in faces {
        if face.score >= options.min_score && face.w.min(face.h) >= options.min_face_size {
            summary.face_count += 1;
            summary.max_score = summary.max_score.max(face.score);
        }
    }
    Ok(summary)
}

fn map_face(
    face: Face,
    prepared: &super::preprocess::Prepared,
    offset_x: u32,
    offset_y: u32,
) -> Face {
    let scale = prepared.scale;
    Face {
        x: offset_x as f32 + face.x / scale,
        y: offset_y as f32 + face.y / scale,
        w: face.w / scale,
        h: face.h / scale,
        score: face.score,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, Rgb, RgbImage};

    #[test]
    fn a_blank_image_has_no_faces() {
        let image = DynamicImage::ImageRgb8(RgbImage::from_pixel(320, 240, Rgb([0, 0, 0])));
        let summary = detect(&image, DetectOptions::default()).expect("detect");
        assert_eq!(summary.face_count, 0);
    }

    #[test]
    fn a_portrait_has_one_face() {
        let bytes = include_bytes!("../../tests/fixtures/face_tags/portrait.jpg");
        let image = image::load_from_memory(bytes).expect("fixture");
        let summary = detect(&image, DetectOptions::default()).expect("detect");
        assert_eq!(summary.face_count, 1, "{summary:?}");
    }

    #[test]
    fn a_composite_with_two_portraits_has_two_faces() {
        let bytes = include_bytes!("../../tests/fixtures/face_tags/portrait.jpg");
        let portrait = image::load_from_memory(bytes).expect("fixture").to_rgb8();
        let (w, h) = portrait.dimensions();
        let mut canvas = RgbImage::from_pixel(w * 2, h, Rgb([0, 0, 0]));
        image::imageops::overlay(&mut canvas, &portrait, 0, 0);
        image::imageops::overlay(&mut canvas, &portrait, w as i64, 0);
        let summary =
            detect(&DynamicImage::ImageRgb8(canvas), DetectOptions::default()).expect("detect");
        assert_eq!(summary.face_count, 2, "{summary:?}");
    }
}
