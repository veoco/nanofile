//! Compliance checks for every embedded model. These do not exercise the
//! pipelines; they make a missing/renamed model, an unreviewed licence, a
//! changed hash, or a stale generated inference plan a test failure rather than
//! a runtime surprise.

use std::path::PathBuf;

use sha2::{Digest, Sha256};

fn models_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models")
}

fn generated_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("inference")
        .join("src")
        .join("generated")
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

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Pull `("/../server/models/<file>")` and `source_sha256: "<digest>"` out of
/// one generated plan module. The generated files are plain data; reading them
/// here keeps the check independent of the inference crate.
fn plan_provenance(text: &str) -> (String, String) {
    let marker = "include_bytes!";
    let start = text.find(marker).expect("include_bytes model path") + marker.len();
    let rest = &text[start..];
    let marker = "models/";
    let offset = rest.find(marker).expect("model path") + marker.len();
    let rest = &rest[offset..];
    let end = rest.find('"').expect("closing quote");
    let file = rest[..end].to_string();

    let marker = "source_sha256: \"";
    let start = text.find(marker).expect("source digest") + marker.len();
    let rest = &text[start..];
    let end = rest.find('"').expect("closing digest quote");
    (file, rest[..end].to_string())
}

#[test]
fn every_embedded_model_matches_the_manifest() {
    let dir = models_dir();
    let manifest = manifest();
    let models = manifest["models"].as_object().expect("models object");
    assert_eq!(models.len(), 6, "manifest must name every embedded asset");
    for (name, entry) in models {
        let bytes = std::fs::read(dir.join(entry["file"].as_str().expect("file")))
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(
            bytes.len() as u64,
            entry["bytes"].as_u64().expect("bytes"),
            "{name}: manifest size"
        );
        assert_eq!(
            digest(&bytes),
            entry["sha256"].as_str().expect("sha256"),
            "{name}"
        );

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

/// The exporter reads the ONNX files at development time and commits a plan per
/// model. This pins the plan to the model bytes: if a model is refreshed without
/// rerunning `tools/models/export_plan.py`, the test fails here instead of the
/// binary silently loading weights at the wrong offsets.
#[test]
fn the_generated_plans_match_the_embedded_models() {
    let dir = generated_dir();
    let manifest = manifest();
    let mut plan_files = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("generated directory") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("rs")
            || path.file_name().and_then(|name| name.to_str()) == Some("mod.rs")
        {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("generated plan");
        assert!(text.contains("static OPS: &[Op]"), "{}", path.display());
        assert!(
            text.contains("static WEIGHTS: &[WeightRef]"),
            "{}",
            path.display()
        );
        assert!(
            text.contains("static OUTPUTS: &[u16]"),
            "{}",
            path.display()
        );
        let (file, recorded) = plan_provenance(&text);
        let model_path = models_dir().join(&file);
        let bytes = std::fs::read(&model_path)
            .unwrap_or_else(|error| panic!("{}: {error}", model_path.display()));
        assert_eq!(
            recorded,
            digest(&bytes),
            "{file}: the plan was generated from different model bytes; \
             rerun `python3 tools/models/export_plan.py`"
        );
        plan_files.push(file);
    }
    assert_eq!(plan_files.len(), 5, "one plan per embedded ONNX graph");

    let mut expected: Vec<String> = manifest["models"]
        .as_object()
        .expect("models object")
        .values()
        .map(|entry| entry["file"].as_str().expect("file").to_string())
        .filter(|file| file.ends_with(".onnx"))
        .collect();
    expected.sort();
    plan_files.sort();
    assert_eq!(plan_files, expected, "the manifest and the plans disagree");
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
