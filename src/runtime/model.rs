//! Data-only deployment descriptors shared by the daemon and native worker.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, io, path::Path};

pub const FORMAT: &str = "e2em-onnx-policy-cross-encoder-v1";
pub const MAX_MODEL_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Asset {
    pub bytes: u64,
    pub sha256: String,
    pub url: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format: String,
    pub id: String,
    pub version: String,
    pub source: String,
    pub source_revision: String,
    pub license: String,
    pub max_tokens: usize,
    pub tail_tokens: usize,
    pub evidence_format: String,
    pub files: BTreeMap<String, Asset>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Descriptor {
    pub manifest: Manifest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Threshold {
    pub wording: String,
    pub action: f64,
    pub review: f64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub model: String,
    pub tokenizer: String,
    pub max_tokens: Option<usize>,
    pub model_categories: bool,
    pub custom_policies: bool,
    #[serde(default)]
    pub thresholds: Vec<Threshold>,
}
pub fn name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
pub fn hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn digest(path: &Path) -> io::Result<String> {
    use std::io::Read;
    let mut input = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
impl Manifest {
    pub fn validate(&self) -> io::Result<()> {
        if self.format != FORMAT
            || !name(&self.id)
            || self.version.is_empty()
            || self.version.len() > 64
            || !self
                .version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-+".contains(&b))
            || self.license.is_empty()
            || self.license.len() > 128
            || self.source.len() > 2048
            || self.source_revision.len() > 128
            || self.max_tokens != 512
            || self.tail_tokens > 256
            || self.evidence_format != "target-first"
            || self.files.len() > 16
            || !self.files.contains_key("model.onnx")
            || !self.files.contains_key("tokenizer.json")
        {
            return Err(io::Error::other("incompatible model deployment descriptor"));
        }
        for (filename, asset) in &self.files {
            if filename.is_empty()
                || filename.len() > 64
                || filename.starts_with('.')
                || !filename
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
                || asset.bytes == 0
                || asset.bytes
                    > if filename == "model.onnx" {
                        MAX_MODEL_BYTES
                    } else {
                        16 * 1024 * 1024
                    }
                || !hex(&asset.sha256, 64)
                || !asset.url.starts_with("https://")
                || asset.url.len() > 2048
            {
                return Err(io::Error::other("invalid model asset"));
            }
        }
        Ok(())
    }
    pub fn identity(&self) -> String {
        format!(
            "{}@{}:{}",
            self.id,
            self.version,
            &self.files["model.onnx"].sha256[..12]
        )
    }
    pub fn verify(&self, directory: &Path) -> io::Result<()> {
        self.validate()?;
        for (filename, asset) in &self.files {
            let path = directory.join(filename);
            let metadata = std::fs::symlink_metadata(&path)?;
            if !metadata.is_file()
                || metadata.len() != asset.bytes
                || digest(&path)? != asset.sha256
            {
                return Err(io::Error::other("model asset size or SHA-256 mismatch"));
            }
        }
        Ok(())
    }
    pub fn metadata(&self, directory: &Path) -> io::Result<Metadata> {
        let mut thresholds = Vec::new();
        if self.files.contains_key("preset.json") {
            let entries: serde_json::Value =
                serde_json::from_slice(&std::fs::read(directory.join("preset.json"))?)
                    .map_err(io::Error::other)?;
            for entry in entries["presets"]
                .as_array()
                .ok_or_else(|| io::Error::other("invalid model thresholds"))?
            {
                let wording = entry["wording"]
                    .as_str()
                    .ok_or_else(|| io::Error::other("invalid threshold wording"))?;
                let action = entry["thresholds"]["action"]
                    .as_f64()
                    .ok_or_else(|| io::Error::other("invalid threshold"))?;
                let review = entry["thresholds"]["review"]
                    .as_f64()
                    .ok_or_else(|| io::Error::other("invalid threshold"))?;
                if wording.is_empty()
                    || wording.len() > 512
                    || !(0.0..=1.0).contains(&review)
                    || !(0.0..=1.0).contains(&action)
                {
                    return Err(io::Error::other("invalid threshold"));
                }
                thresholds.push(Threshold {
                    wording: wording.into(),
                    action,
                    review,
                });
            }
        }
        Ok(Metadata {
            model: self.identity(),
            tokenizer: format!("sha256:{}", &self.files["tokenizer.json"].sha256[..12]),
            max_tokens: Some(self.max_tokens),
            model_categories: true,
            custom_policies: true,
            thresholds,
        })
    }
}
