//! Compliance checks for the embedded YuNet model. These do not run inference;
//! they make a missing/renamed model, an unreviewed licence or a changed hash a
//! test failure rather than a runtime surprise.

#![cfg(feature = "face-tags")]

use std::path::PathBuf;

use sha2::{Digest, Sha256};

fn models_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models")
}

#[test]
fn the_embedded_model_matches_the_manifest() {
    let dir = models_dir();
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).expect("manifest"))
            .expect("manifest JSON");
    let model = &manifest["models"]["yunet"];
    let bytes = std::fs::read(dir.join(model["file"].as_str().expect("file"))).expect("weights");
    assert_eq!(
        bytes.len() as u64,
        model["bytes"].as_u64().expect("bytes"),
        "manifest size"
    );
    let digest: String = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(digest, model["sha256"].as_str().expect("sha256"));
    assert_eq!(model["license"], "MIT");

    let license =
        std::fs::read_to_string(dir.join(model["license_file"].as_str().expect("license_file")))
            .expect("license");
    assert!(license.contains("MIT License"));
    assert!(license.contains("Shiqi Yu"));
}

#[test]
fn the_weights_are_f32_safetensors_with_the_expected_heads() {
    let bytes = std::fs::read(models_dir().join("yunet.safetensors")).expect("weights");
    let tensors = safetensors::SafeTensors::deserialize(&bytes).expect("safetensors");
    let names: Vec<&str> = tensors.names().into_iter().collect();
    assert_eq!(names.len(), 106, "53 convs, weight + bias");
    for level in 0..3 {
        assert!(names.contains(&format!("head.cls.{level}.conv1.weight").as_str()));
        assert!(names.contains(&format!("head.bbox.{level}.conv2.weight").as_str()));
        assert!(names.contains(&format!("head.obj.{level}.conv1.weight").as_str()));
    }
    for name in names {
        let tensor = tensors.tensor(name).expect("tensor");
        assert_eq!(tensor.dtype(), safetensors::Dtype::F32, "{name}");
    }
}
