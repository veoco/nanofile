//! Compliance checks for every embedded model. These do not exercise the
//! pipelines; they make a missing/renamed model, an unreviewed licence, a
//! changed hash or a graph tract cannot load a test failure rather than a
//! runtime surprise.

use std::path::PathBuf;

use sha2::{Digest, Sha256};
use tract::prelude::*;

fn models_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models")
}

fn manifest() -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(models_dir().join("manifest.json")).expect("manifest"))
        .expect("manifest JSON")
}

fn model(name: &str) -> (serde_json::Value, Vec<u8>) {
    let dir = models_dir();
    let manifest = manifest();
    let entry = manifest["models"][name].clone();
    let bytes = std::fs::read(dir.join(entry["file"].as_str().expect("file"))).expect("model");
    (entry, bytes)
}

#[test]
fn every_embedded_model_matches_the_manifest() {
    let dir = models_dir();
    let manifest = manifest();
    let models = manifest["models"].as_object().expect("models object");
    assert_eq!(models.len(), 4, "manifest must name every embedded asset");
    for (name, entry) in models {
        let bytes = std::fs::read(dir.join(entry["file"].as_str().expect("file")))
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(
            bytes.len() as u64,
            entry["bytes"].as_u64().expect("bytes"),
            "{name}: manifest size"
        );
        let digest: String = Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(digest, entry["sha256"].as_str().expect("sha256"), "{name}");

        let license =
            std::fs::read_to_string(dir.join(entry["license_file"].as_str().expect("license")))
                .unwrap_or_else(|error| panic!("{name}: {error}"));
        match entry["license"].as_str().expect("license") {
            "MIT" => {
                assert!(license.contains("MIT License"), "{name}");
                assert!(license.contains("Shiqi Yu"), "{name}");
            }
            "Apache-2.0" => {
                assert!(license.contains("Apache License"), "{name}");
                assert!(license.contains("Version 2.0"), "{name}");
            }
            other => panic!("{name}: unrecognised licence {other}"),
        }
    }
}

#[test]
fn the_ocr_dictionary_has_one_character_per_line_and_the_models_classes() {
    let (_, bytes) = model("ppocr_v6_tiny_keys");
    let text = String::from_utf8(bytes).expect("UTF-8 dictionary");
    let characters: Vec<&str> = text.lines().collect();
    assert_eq!(characters.len(), 6904);
    assert_eq!(characters.len() + 2, 6906, "blank + trailing space");
    for character in &characters {
        assert_eq!(
            character.chars().count(),
            1,
            "dictionary entry is not one character: {character:?}"
        );
    }
    let mut unique = characters.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), characters.len(), "duplicate dictionary entry");
}

fn load_plan(bytes: &[u8], input: &str) -> tract::Runnable {
    let mut model = tract::onnx()
        .expect("onnx framework")
        .load_buffer_with_options(bytes, OnnxOptions::new().ignore_value_info(true))
        .expect("load the ONNX graph");
    model.set_input_fact(0, input).expect("input fact");
    let outputs = model.output_count().expect("output count");
    for output in 0..outputs {
        model.set_output_fact(output, ()).expect("output fact");
    }
    let typed = model.into_model().expect("typed model");
    tract::runtime_for_name("cpu")
        .expect("cpu runtime")
        .prepare(typed)
        .expect("prepare")
}

fn run_zeros(plan: &tract::Runnable, dims: &[usize]) -> Vec<tract::Tensor> {
    let values = vec![0f32; dims.iter().product()];
    let input = tract::Tensor::from_slice(dims, &values).expect("input tensor");
    plan.run([input]).expect("run")
}

#[test]
fn the_yunet_graph_loads_and_declares_its_twelve_heads() {
    let (_, bytes) = model("yunet");
    let plan = load_plan(&bytes, "1,3,640,640,f32");
    let outputs = run_zeros(&plan, &[1, 3, 640, 640]);
    assert_eq!(outputs.len(), 12, "cls/obj/bbox/kps x three strides");
    let shapes: Vec<Vec<usize>> = outputs
        .iter()
        .map(|tensor| tensor.shape().expect("shape").to_vec())
        .collect();
    // cls and obj: [1, anchors, 1]; bbox: [1, anchors, 4]; kps: [1, anchors, 10].
    for (level, anchors) in [6400usize, 1600, 400].into_iter().enumerate() {
        assert_eq!(shapes[level], vec![1, anchors, 1], "cls {level}");
        assert_eq!(shapes[3 + level], vec![1, anchors, 1], "obj {level}");
        assert_eq!(shapes[6 + level], vec![1, anchors, 4], "bbox {level}");
        assert_eq!(shapes[9 + level], vec![1, anchors, 10], "kps {level}");
    }
}

#[test]
fn the_ocr_detection_graph_loads_and_returns_a_probability_map() {
    let (_, bytes) = model("ppocr_v6_tiny_det");
    let plan = load_plan(&bytes, "1,3,64,64,f32");
    let outputs = run_zeros(&plan, &[1, 3, 64, 64]);
    assert_eq!(outputs.len(), 1);
    assert_eq!(
        outputs[0].shape().expect("shape").to_vec(),
        vec![1, 1, 64, 64],
        "DB output is one probability map"
    );
}

#[test]
fn the_ocr_recognition_graph_loads_and_returns_ctc_classes() {
    let (_, bytes) = model("ppocr_v6_tiny_rec");
    let plan = load_plan(&bytes, "1,3,48,320,f32");
    let outputs = run_zeros(&plan, &[1, 3, 48, 320]);
    assert_eq!(outputs.len(), 1);
    let shape = outputs[0].shape().expect("shape");
    assert_eq!(shape.len(), 3, "{shape:?}");
    assert_eq!(shape[0], 1);
    assert!(shape[1] > 0);
    assert_eq!(shape[2], 6906, "CTC blank + dictionary + space");
}
