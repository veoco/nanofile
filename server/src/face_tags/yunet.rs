//! YuNet 2023mar, run as the published ONNX graph through tract.
//!
//! The graph is tiny: a Conv/ReLU/MaxPool backbone, a three-level FPN, and four
//! heads per level. The ONNX export already flattens each head to
//! `[1, anchors, channels]` and applies sigmoid to cls/obj; this module only
//! decodes the boxes. The landmarks are not read because the tag pipeline needs
//! only boxes and scores.
//!
//! The published graph has a dynamic input shape; the caller letterboxes to
//! 640x640, so this module prepares one plan for that one shape.

use crate::inference;

use super::preprocess::{INPUT_SIZE, Prepared};

/// Embedded MIT-licensed YuNet weights, the upstream ONNX file unchanged.
pub const WEIGHTS: &[u8] = include_bytes!("../../models/yunet.onnx");

const STRIDES: [usize; 3] = [8, 16, 32];
/// Number of outputs the published graph declares.
const OUTPUT_COUNT: usize = 12;

/// A decoded face in **prepared-input** coordinates (640x640 canvas).
#[derive(Clone, Copy, Debug)]
pub struct Face {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub score: f32,
}

pub struct YuNet {
    plan: tract::Runnable,
}

impl YuNet {
    pub fn load() -> Result<Self, String> {
        let model = inference::OnnxModel::load(WEIGHTS)?;
        let spec = format!("1,3,{INPUT_SIZE},{INPUT_SIZE},f32");
        let plan = model.plan(&spec)?;
        Ok(Self { plan })
    }

    /// Run the detector on a prepared 640x640 BGR input.
    pub fn forward(&self, prepared: &Prepared) -> Result<Vec<Face>, String> {
        let dims = [1, 3, INPUT_SIZE as usize, INPUT_SIZE as usize];
        let outputs = inference::run(&self.plan, &dims, &prepared.data)?;
        if outputs.len() < OUTPUT_COUNT {
            return Err(format!(
                "the YuNet graph returned {} outputs, expected {OUTPUT_COUNT}",
                outputs.len()
            ));
        }

        // The ONNX output list is cls_8, cls_16, cls_32, obj_8, obj_16,
        // obj_32, bbox_8, bbox_16, bbox_32, kps_8, kps_16, kps_32.
        let mut faces = Vec::new();
        for (level, stride) in STRIDES.iter().enumerate() {
            decode_level(
                &outputs[level],
                &outputs[3 + level],
                &outputs[6 + level],
                *stride,
                &mut faces,
            )?;
        }
        nms(&mut faces, 0.3, 100);
        Ok(faces)
    }
}

/// Decode one FPN level's cls/obj/bbox heads into boxes.
///
/// The ONNX export already applied sigmoid to cls and obj and left the bbox
/// channels raw, so the width/height are exponentiated here.
fn decode_level(
    cls: &tract::Tensor,
    obj: &tract::Tensor,
    bbox: &tract::Tensor,
    stride: usize,
    faces: &mut Vec<Face>,
) -> Result<(), String> {
    let rows = INPUT_SIZE as usize / stride;
    let cols = rows;
    let anchors = rows * cols;

    let cls_shape = inference::shape(cls)?;
    let obj_shape = inference::shape(obj)?;
    let bbox_shape = inference::shape(bbox)?;
    if cls_shape.len() != 3 || cls_shape[0] != 1 || cls_shape[2] != 1 || cls_shape[1] != anchors {
        return Err(format!(
            "the YuNet cls output for stride {stride} has shape {cls_shape:?}, expected [1, {anchors}, 1]"
        ));
    }
    if obj_shape != cls_shape {
        return Err(format!(
            "the YuNet obj output for stride {stride} has shape {obj_shape:?}, expected {cls_shape:?}"
        ));
    }
    if bbox_shape.len() != 3 || bbox_shape[0] != 1 || bbox_shape[1] != anchors || bbox_shape[2] != 4
    {
        return Err(format!(
            "the YuNet bbox output for stride {stride} has shape {bbox_shape:?}, expected [1, {anchors}, 4]"
        ));
    }

    let cls = inference::values(cls)?;
    let obj = inference::values(obj)?;
    let bbox = inference::values(bbox)?;

    for row in 0..rows {
        for col in 0..cols {
            let idx = row * cols + col;
            let score = (cls[idx].clamp(0.0, 1.0) * obj[idx].clamp(0.0, 1.0)).sqrt();
            if score < 0.05 {
                continue;
            }
            let b = &bbox[idx * 4..idx * 4 + 4];
            let cx = (col as f32 + b[0]) * stride as f32;
            let cy = (row as f32 + b[1]) * stride as f32;
            let w = b[2].exp() * stride as f32;
            let h = b[3].exp() * stride as f32;
            faces.push(Face {
                x: cx - w / 2.0,
                y: cy - h / 2.0,
                w,
                h,
                score,
            });
        }
    }
    Ok(())
}

pub(crate) fn nms(faces: &mut Vec<Face>, threshold: f32, top_k: usize) {
    faces.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut kept: Vec<Face> = Vec::new();
    for face in faces.drain(..) {
        if kept.len() >= top_k {
            break;
        }
        if kept.iter().all(|other| iou(&face, other) < threshold) {
            kept.push(face);
        }
    }
    *faces = kept;
}

fn iou(a: &Face, b: &Face) -> f32 {
    let x1 = a.x.max(b.x);
    let y1 = a.y.max(b.y);
    let x2 = (a.x + a.w).min(b.x + b.w);
    let y2 = (a.y + a.h).min(b.y + b.h);
    let inter = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
    let union = a.w * a.h + b.w * b.h - inter;
    if union <= 0.0 { 0.0 } else { inter / union }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iou_of_identical_boxes_is_one() {
        let a = Face {
            x: 1.0,
            y: 2.0,
            w: 10.0,
            h: 20.0,
            score: 0.9,
        };
        let b = a;
        assert!((iou(&a, &b) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn the_portrait_decodes_to_a_plausible_box() {
        let bytes = include_bytes!("../../tests/fixtures/face_tags/portrait.jpg");
        let image = image::load_from_memory(bytes).expect("fixture");
        let prepared = super::super::preprocess::prepare(&image).expect("prepared");
        let model = YuNet::load().expect("model");
        let faces = model.forward(&prepared).expect("forward");
        let best = faces
            .iter()
            .max_by(|a, b| a.score.partial_cmp(&b.score).expect("scores"))
            .expect("a face");
        // The ONNX heads are already sigmoided; the box is decoded from the
        // bbox head, so this catches an output-order or exp() regression.
        assert!(best.score > 0.9, "{best:?}");
        assert!(best.w > 50.0 && best.h > 50.0, "{best:?}");
        assert!(
            best.x >= 0.0 && best.y >= 0.0 && best.x + best.w <= 640.0 && best.y + best.h <= 640.0,
            "{best:?}"
        );
    }

    #[test]
    fn nms_keeps_the_best_of_overlapping_boxes() {
        let mut faces = vec![
            Face {
                x: 0.0,
                y: 0.0,
                w: 10.0,
                h: 10.0,
                score: 0.5,
            },
            Face {
                x: 1.0,
                y: 1.0,
                w: 10.0,
                h: 10.0,
                score: 0.9,
            },
            Face {
                x: 100.0,
                y: 100.0,
                w: 10.0,
                h: 10.0,
                score: 0.8,
            },
        ];
        nms(&mut faces, 0.3, 10);
        assert_eq!(faces.len(), 2);
        assert_eq!(faces[0].score, 0.9);
    }
}
