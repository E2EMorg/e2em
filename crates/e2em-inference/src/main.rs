//! Native worker with automatic accelerator selection and CPU fallback.
mod accelerator;
use clap::Parser;
use e2em_runtime::{
    BackendError, PolicyScorer,
    runtime::{
        model::{Descriptor, Manifest, Metadata},
        process::serve_worker,
        resources::Resources,
    },
};
use ort::value::Tensor;
use std::{path::PathBuf, sync::Mutex};
use tokenizers::{Encoding, Tokenizer};

#[derive(Parser)]
#[command(version, about = "Local E2EM ONNX inference worker")]
struct Args {
    #[arg(long)]
    model: PathBuf,
    #[arg(long)]
    library: PathBuf,
    #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u16).range(1..=16))]
    threads: u16,
    /// Prefer a usable GPU, force CPU, or require GPU initialization.
    #[arg(long, value_enum, default_value_t = accelerator::Device::Auto)]
    device: accelerator::Device,
    /// Load and score a fixed probe, print backend diagnostics, then exit.
    #[arg(long)]
    device_status: bool,
    /// Save ONNX execution profiling for the fixed diagnostic probe.
    #[arg(long, requires = "device_status")]
    profile: Option<PathBuf>,
}
struct Scorer {
    session: Mutex<Option<accelerator::Loaded>>,
    options: accelerator::Options,
    tokenizer: Tokenizer,
    manifest: Manifest,
    metadata: Metadata,
    pad: u32,
    coverage: Mutex<(bool, bool)>,
}
fn failure(_: impl std::fmt::Display) -> BackendError {
    BackendError::new("local model inference failed")
}
fn infer(
    loaded: &mut accelerator::Loaded,
    rows: usize,
    width: usize,
    ids: &[i64],
    masks: &[i64],
) -> Result<Vec<f32>, BackendError> {
    loaded.admit_batch().map_err(failure)?;
    let outputs = loaded.session.run(ort::inputs![
        "input_ids" => Tensor::from_array(([rows, width], ids.to_vec())).map_err(failure)?,
        "attention_mask" => Tensor::from_array(([rows, width], masks.to_vec())).map_err(failure)?,
    ]).map_err(failure)?;
    let (shape, values) = outputs
        .get("logits")
        .ok_or_else(|| failure("missing model logits"))?
        .try_extract_tensor::<f32>()
        .map_err(failure)?;
    if shape.as_ref() != [rows as i64, 1]
        || values.len() != rows
        || !values.iter().all(|v| v.is_finite())
    {
        return Err(failure("incompatible model output"));
    }
    Ok(values.to_vec())
}
fn retain(encoding: &Encoding, budget: usize, tail: usize) -> Encoding {
    let length = encoding.len();
    let positions: Vec<usize> = if length <= budget {
        (0..length).collect()
    } else {
        let tail = tail.min(budget / 2);
        (0..budget - tail).chain(length - tail..length).collect()
    };
    Encoding::new(
        positions.iter().map(|&i| encoding.get_ids()[i]).collect(),
        positions
            .iter()
            .map(|&i| encoding.get_type_ids()[i])
            .collect(),
        positions
            .iter()
            .map(|&i| encoding.get_tokens()[i].clone())
            .collect(),
        positions
            .iter()
            .map(|&i| encoding.get_word_ids()[i])
            .collect(),
        positions
            .iter()
            .map(|&i| encoding.get_offsets()[i])
            .collect(),
        positions
            .iter()
            .map(|&i| encoding.get_special_tokens_mask()[i])
            .collect(),
        positions
            .iter()
            .map(|&i| encoding.get_attention_mask()[i])
            .collect(),
        vec![],
        Default::default(),
    )
}
impl Scorer {
    fn encode(
        &self,
        message: &str,
        context: Option<&str>,
        policies: &[String],
    ) -> Result<Vec<Encoding>, BackendError> {
        let context = context.map(str::trim).filter(|s| !s.is_empty());
        let evidence = context.map_or_else(
            || message.into(),
            |c| format!("Message:\n{message}\n\nContext:\n{c}"),
        );
        let evidence = self.tokenizer.encode(evidence, false).map_err(failure)?;
        let probe = self.tokenizer.encode("a", false).map_err(failure)?;
        let special_count = self
            .tokenizer
            .post_process(probe.clone(), Some(probe.clone()), true)
            .map_err(failure)?
            .len()
            - probe.len() * 2;
        let mut result = Vec::with_capacity(policies.len());
        let mut coverage = (true, true);
        for policy in policies {
            let first = self
                .tokenizer
                .encode(policy.as_str(), false)
                .map_err(failure)?;
            let budget = self
                .manifest
                .max_tokens
                .checked_sub(first.len() + special_count)
                .filter(|&b| b > 0)
                .ok_or_else(|| failure("policy exceeds token budget"))?;
            if evidence.len() > budget {
                coverage.0 = false;
                if context.is_some() {
                    coverage.1 = false;
                }
            }
            let second = retain(&evidence, budget, self.manifest.tail_tokens);
            result.push(
                self.tokenizer
                    .post_process(first, Some(second), true)
                    .map_err(failure)?,
            );
        }
        *self.coverage.lock().map_err(failure)? = coverage;
        Ok(result)
    }
}
impl PolicyScorer for Scorer {
    fn supports_model_categories(&self) -> bool {
        true
    }
    fn supports_custom_policies(&self) -> bool {
        true
    }
    fn model_version(&self) -> String {
        self.metadata.model.clone()
    }
    fn tokenizer_version(&self) -> String {
        self.metadata.tokenizer.clone()
    }
    fn max_tokens(&self) -> Option<usize> {
        self.metadata.max_tokens
    }
    fn coverage(&self) -> (bool, bool) {
        self.coverage.lock().map_or((false, false), |c| *c)
    }
    fn score(
        &self,
        message: &str,
        context: Option<&str>,
        policy: &str,
    ) -> Result<f64, BackendError> {
        self.score_many(message, context, &[policy.into()])
            .map(|scores| scores[0])
    }
    fn score_many(
        &self,
        message: &str,
        context: Option<&str>,
        policies: &[String],
    ) -> Result<Vec<f64>, BackendError> {
        if policies.is_empty()
            || policies.len() > 64
            || policies.iter().any(|p| p.is_empty() || p.len() > 512)
        {
            return Err(failure("invalid model policies"));
        }
        let encoded = self.encode(message, context, policies)?;
        let pad = self.pad;
        let mut session = self.session.lock().map_err(failure)?;
        let mut scores = Vec::with_capacity(policies.len());
        for batch in encoded.chunks(8) {
            Resources::detect()
                .and_then(Resources::admit_inference)
                .map_err(failure)?;
            let width = batch
                .iter()
                .map(Encoding::len)
                .max()
                .ok_or_else(|| failure("empty batch"))?;
            let mut ids = vec![i64::from(pad); batch.len() * width];
            let mut masks = vec![0i64; ids.len()];
            for (row, encoding) in batch.iter().enumerate() {
                for (column, &id) in encoding.get_ids().iter().enumerate() {
                    ids[row * width + column] = i64::from(id);
                    masks[row * width + column] = 1;
                }
            }
            let values = accelerator::retry_on_cpu(
                &mut session,
                self.options.device == accelerator::Device::Auto,
                |loaded| loaded.provider != "CPU",
                |loaded| infer(loaded, batch.len(), width, &ids, &masks),
                || {
                    let mut cpu = accelerator::cpu(&self.options).map_err(failure)?;
                    cpu.fallback_reasons
                        .push("GPU execution failed; rebuilt CPU session".into());
                    Ok(cpu)
                },
                || failure("session unavailable"),
            )?;
            scores.extend(values.iter().map(|&value| {
                let logit = f64::from(value);
                if logit >= 0.0 {
                    1.0 / (1.0 + (-logit).exp())
                } else {
                    let exp = logit.exp();
                    exp / (1.0 + exp)
                }
            }));
        }
        Ok(scores)
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    if !args.library.is_absolute() || !args.model.is_absolute() {
        return Err("worker paths must be absolute".into());
    }
    let descriptor: Descriptor =
        serde_json::from_slice(&std::fs::read(args.model.join("model.json"))?)?;
    descriptor.manifest.validate()?;
    // Also protect direct worker invocation, before asset hashing or ONNX load.
    let plan = Resources::detect()?.model_plan(
        descriptor.manifest.files["model.onnx"].bytes,
        usize::from(args.threads),
    )?;
    descriptor.manifest.verify(&args.model)?;
    let metadata = descriptor.manifest.metadata(&args.model)?;
    // The library path comes from trusted installation config, never the model URL.
    ort::init_from(&args.library)?
        .with_name("e2em-inference")
        .commit();
    let options = accelerator::options(
        &args.model,
        descriptor.manifest.files["model.onnx"].bytes,
        plan.threads,
        args.device,
        args.profile,
    );
    let session = accelerator::load(&options)?;
    let mut tokenizer =
        Tokenizer::from_file(args.model.join("tokenizer.json")).map_err(|_| "invalid tokenizer")?;
    let pad = tokenizer
        .get_padding()
        .map(|p| p.pad_id)
        .or_else(|| tokenizer.token_to_id("[PAD]"))
        .or_else(|| tokenizer.token_to_id("<pad>"))
        .ok_or("tokenizer has no padding token")?;
    tokenizer
        .with_truncation(None)
        .map_err(|_| "invalid tokenizer truncation")?;
    tokenizer.with_padding(None);
    let scorer = Scorer {
        session: Mutex::new(Some(session)),
        options,
        tokenizer,
        manifest: descriptor.manifest,
        metadata,
        pad,
        coverage: Mutex::new((true, true)),
    };
    // Registration alone cannot establish that a GPU can execute this graph.
    // Warm up before announcing readiness so Auto can recover on CPU.
    let probe = scorer.score_many(
        "Thank you for your help.",
        None,
        &["Do not threaten to injure, kill, or otherwise harm a person.".into()],
    )?;
    if args.device_status {
        let mut loaded = scorer.session.lock().map_err(failure)?;
        let loaded = loaded.as_mut().ok_or("session unavailable")?;
        let profile = if scorer.options.profile.is_some() {
            Some(loaded.session.end_profiling()?)
        } else {
            None
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "requested_device": format!("{:?}", scorer.options.device).to_lowercase(),
                "selected_provider": loaded.provider,
                "fallback_reasons": loaded.fallback_reasons,
                "cpu_threads": plan.threads,
                "probe_probability": probe[0],
                "profile": profile,
            }))?
        );
        return Ok(());
    }
    serve_worker(&scorer)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn truncation_retains_last_evidence_and_bounds_tail() {
        let original = Encoding::new(
            (0..10).collect(),
            vec![0; 10],
            vec![String::new(); 10],
            vec![None; 10],
            vec![(0, 0); 10],
            vec![0; 10],
            vec![1; 10],
            vec![],
            Default::default(),
        );
        assert_eq!(retain(&original, 6, 64).get_ids(), &[0, 1, 2, 7, 8, 9]);
        assert_eq!(retain(&original, 10, 64).get_ids(), original.get_ids());
    }
}
