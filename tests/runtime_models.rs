use e2em_runtime::runtime::model::{Asset, Descriptor, FORMAT, Manifest};
use std::collections::BTreeMap;
fn descriptor() -> Descriptor {
    let asset = Asset {
        bytes: 4,
        sha256: "a".repeat(64),
        url: "https://example.org/model".into(),
    };
    Descriptor {
        signature: None,
        manifest: Manifest {
            format: FORMAT.into(),
            id: "custom".into(),
            version: "1.0.0".into(),
            source: "https://example.org".into(),
            source_revision: "revision".into(),
            license: "MIT".into(),
            max_tokens: 512,
            tail_tokens: 64,
            evidence_format: "target-first".into(),
            files: BTreeMap::from([
                ("model.onnx".into(), asset.clone()),
                ("tokenizer.json".into(), asset),
            ]),
        },
    }
}
#[test]
fn deployment_rejects_escaping_files_unbounded_sizes_and_plaintext_urls() {
    let valid = descriptor();
    valid.manifest.validate().unwrap();
    for bad in ["../outside", ".hidden", "a/b", "a\\b", ""] {
        let mut value = valid.clone();
        value
            .manifest
            .files
            .insert(bad.into(), value.manifest.files["model.onnx"].clone());
        assert!(value.manifest.validate().is_err());
    }
    let mut value = valid.clone();
    value.manifest.files.get_mut("model.onnx").unwrap().bytes = u64::MAX;
    assert!(value.manifest.validate().is_err());
    let mut value = valid.clone();
    value.manifest.files.get_mut("model.onnx").unwrap().url = "http://example.org/model".into();
    assert!(value.manifest.validate().is_err());
    let mut value = valid;
    value.manifest.max_tokens = usize::MAX;
    assert!(value.manifest.validate().is_err());
}
#[test]
fn deployment_rejects_corrupted_assets_before_loading() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("model.onnx"), b"data").unwrap();
    std::fs::write(directory.path().join("tokenizer.json"), b"data").unwrap();
    let mut value = descriptor();
    for asset in value.manifest.files.values_mut() {
        asset.sha256 =
            e2em_runtime::runtime::model::digest(&directory.path().join("model.onnx")).unwrap();
    }
    value.manifest.verify(directory.path()).unwrap();
    std::fs::write(directory.path().join("model.onnx"), b"evil").unwrap();
    assert!(value.manifest.verify(directory.path()).is_err());
}
#[cfg(feature = "runtime-service")]
#[test]
fn default_model_requires_release_signature_custom_source_is_explicit() {
    let value = descriptor();
    assert!(e2em_runtime::runtime::models::verify_descriptor(&value, true).is_err());
    e2em_runtime::runtime::models::verify_descriptor(&value, false).unwrap();
}
