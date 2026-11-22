//! Runtime tests: plan/model consistency, every embedded model loading, and the
//! dynamic detection and recognition shapes.

use super::*;

use sha2::Digest;

const KINDS: [ModelKind; 5] = [
    ModelKind::YuNet,
    ModelKind::OcrDet,
    ModelKind::OcrRec,
    ModelKind::DocOri,
    ModelKind::TextlineOri,
];

fn plan_bytes(kind: ModelKind) -> &'static [u8] {
    Model::load(kind).expect("model").plan().bytes
}

/// Deterministic, cheap pseudo-random input in `[-1, 1)`.
fn input_values(len: usize, seed: u32) -> Vec<f32> {
    let mut state = seed | 1;
    let mut values = Vec::with_capacity(len);
    for _ in 0..len {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let unit = (state >> 8) as f32 / (1u32 << 24) as f32;
        values.push(unit * 2.0 - 1.0);
    }
    values
}

#[test]
fn plans_are_in_sync_with_the_onnx_files() {
    for kind in KINDS {
        let bytes = plan_bytes(kind);
        let digest = sha2::Sha256::digest(bytes);
        let actual: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        let model = Model::load(kind).expect("model");
        let expected = model.plan().source_sha256;
        assert_eq!(
            actual, expected,
            "{kind:?}: the plan was generated from a different ONNX file; \
             rerun tools/models/export_plan.py"
        );
    }
}

#[test]
fn every_model_loads_and_runs_the_orientation_sized_input() {
    // Zero input still produces well-shaped outputs for every model; this is the
    // same smoke test the startup probe relies on.
    for kind in KINDS {
        let mut model = Model::load(kind).expect("model");
        let dims: &[usize] = match kind {
            ModelKind::YuNet => &[1, 3, 640, 640],
            ModelKind::OcrDet => &[1, 3, 64, 64],
            ModelKind::OcrRec => &[1, 3, 48, 160],
            ModelKind::DocOri => &[1, 3, 224, 224],
            ModelKind::TextlineOri => &[1, 3, 80, 160],
        };
        let input = vec![0f32; dims.iter().product()];
        let mut outputs = Vec::new();
        model.run(dims, &input, &mut outputs).expect("run");
        assert!(!outputs.is_empty(), "{kind:?} produced no outputs");
        for tensor in &outputs {
            assert!(!tensor.data.is_empty(), "{kind:?} produced an empty output");
            assert_eq!(tensor.dims.iter().product::<usize>(), tensor.data.len());
        }
    }
}

#[test]
fn detection_runs_at_dynamic_shapes() {
    let mut model = Model::load(ModelKind::OcrDet).expect("model");
    let mut outputs = Vec::new();
    for dims in [[1usize, 3, 64, 64], [1, 3, 128, 96], [1, 3, 192, 192]] {
        let input = input_values(dims.iter().product(), dims[2] as u32);
        model.run(&dims, &input, &mut outputs).expect("run");
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].dims, vec![1, 1, dims[2], dims[3]]);
    }
}

#[test]
fn recognition_runs_at_every_bucket() {
    let mut model = Model::load(ModelKind::OcrRec).expect("model");
    let mut outputs = Vec::new();
    for bucket in [160usize, 320, 640, 960, 1280] {
        let dims = [1usize, 3, 48, bucket];
        let input = input_values(dims.iter().product(), bucket as u32);
        model.run(&dims, &input, &mut outputs).expect("run");
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].dims.len(), 3);
        assert_eq!(outputs[0].dims[0], 1);
        assert_eq!(
            outputs[0].dims[2],
            crate::ocr::keys::CLASSES,
            "expected the OCR character classes"
        );
    }
}
