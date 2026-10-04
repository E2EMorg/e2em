//! Prefer a usable accelerator, and rebuild a fresh CPU session on failure.
use clap::ValueEnum;
use ort::{
    ep::{self, ExecutionProvider, ExecutionProviderDispatch},
    session::Session,
};
use std::{
    io,
    path::{Path, PathBuf},
};

const GIB: usize = 1024 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum Device {
    #[default]
    Auto,
    Cpu,
    Gpu,
}

pub struct Options {
    pub model: PathBuf,
    pub model_bytes: u64,
    pub threads: usize,
    pub device: Device,
    pub profile: Option<PathBuf>,
}

pub struct Loaded {
    pub session: Session,
    pub provider: &'static str,
    pub fallback_reasons: Vec<String>,
    #[cfg(target_os = "linux")]
    cuda: Option<std::sync::Arc<cudarc::driver::CudaContext>>,
}

impl Loaded {
    pub fn admit_batch(&self) -> io::Result<()> {
        #[cfg(target_os = "linux")]
        if let Some(context) = &self.cuda {
            let (free, _) = context.mem_get_info().map_err(io::Error::other)?;
            if free < 256 * 1024 * 1024 {
                return Err(io::Error::other(
                    "CUDA device has insufficient remaining memory",
                ));
            }
        }
        Ok(())
    }
}

fn build(options: &Options, provider: Option<ExecutionProviderDispatch>) -> ort::Result<Session> {
    let mut builder = Session::builder()?
        .with_intra_threads(options.threads)?
        .with_inter_threads(1)?
        .with_parallel_execution(false)?;
    if let Some(provider) = provider {
        // DirectML requires sequential execution and disabled memory patterns.
        builder = builder
            .with_memory_pattern(false)?
            .with_execution_providers([provider.error_on_failure()])?;
    }
    if let Some(path) = &options.profile {
        builder = builder.with_profiling(path)?;
    }
    builder.commit_from_file(&options.model)
}

fn available(provider: &impl ExecutionProvider) -> bool {
    provider.supported_by_platform() && provider.is_available().unwrap_or(false)
}

// Injecting attempts lets tests cover initialization failures and strict GPU
// mode on hosts with no accelerators, without creating real model sessions.
fn choose<T>(
    device: Device,
    candidates: impl IntoIterator<Item = &'static str>,
    mut gpu: impl FnMut(&'static str) -> Result<T, String>,
    cpu: impl FnOnce() -> Result<T, String>,
) -> Result<(T, Vec<String>), String> {
    let mut failures = Vec::new();
    if device != Device::Cpu {
        for candidate in candidates {
            match gpu(candidate) {
                Ok(value) => return Ok((value, failures)),
                Err(reason) => failures.push(format!("{candidate}: {reason}")),
            }
        }
        if device == Device::Gpu {
            return Err(format!(
                "no usable GPU execution provider: {}",
                failures.join("; ")
            ));
        }
    }
    cpu().map(|value| (value, failures))
}

#[cfg(target_os = "linux")]
fn cuda_contexts(
    model_bytes: u64,
) -> Result<Vec<(std::sync::Arc<cudarc::driver::CudaContext>, usize)>, String> {
    use cudarc::driver::CudaContext;
    // cudarc's dynamic loader panics when the driver library is absent. Catch
    // that boundary so an accelerator-enabled installer still works on CPU hosts.
    std::panic::catch_unwind(|| -> Result<_, String> {
        let count = CudaContext::device_count().map_err(|e| e.to_string())?;
        let required = model_bytes
            .checked_mul(4)
            .and_then(|n| n.checked_add(GIB as u64))
            .ok_or("CUDA memory estimate overflow")?;
        let mut devices = Vec::new();
        for ordinal in 0..count.min(16) {
            let Ok(context) = CudaContext::new(ordinal as usize) else {
                continue;
            };
            let Ok((free, _)) = context.mem_get_info() else {
                continue;
            };
            if free as u64 >= required {
                // Leave 512 MiB for the display/other processes, cap our arena
                // to the conservative workload estimate rather than all VRAM.
                let budget = (free - GIB / 2).min(required as usize);
                devices.push((context, budget));
            }
        }
        devices.sort_by_key(|(_, budget)| std::cmp::Reverse(*budget));
        if devices.is_empty() {
            return Err("no visible CUDA device with enough free VRAM".into());
        }
        Ok(devices)
    })
    .map_err(|_| "CUDA driver could not be loaded".to_string())?
}

pub fn load(options: &Options) -> io::Result<Loaded> {
    let candidates: Vec<&'static str> = if cfg!(target_os = "windows") {
        vec!["DirectML"]
    } else if cfg!(target_os = "macos") {
        vec!["CoreML"]
    } else if cfg!(target_os = "linux") {
        vec!["CUDA", "MIGraphX", "ROCm", "OpenVINO GPU", "WebGPU"]
    } else {
        Vec::new()
    };
    let (mut loaded, failures) = choose(
        options.device,
        candidates,
        |name| {
            let provider = match name {
                "DirectML" if available(&ep::DirectML::default()) => ep::DirectML::default()
                    .with_performance_preference(
                        ep::directml::PerformancePreference::HighPerformance,
                    )
                    .build(),
                "CoreML" if available(&ep::CoreML::default()) => ep::CoreML::default()
                    .with_compute_units(ep::coreml::ComputeUnits::All)
                    .with_low_precision_accumulation_on_gpu(false)
                    .build(),
                #[cfg(target_os = "linux")]
                "CUDA" if available(&ep::CUDA::default()) => {
                    let mut failures = Vec::new();
                    for (context, budget) in cuda_contexts(options.model_bytes)? {
                        let provider = ep::CUDA::default()
                            .with_device_id(context.ordinal() as i32)
                            .with_memory_limit(budget)
                            .with_arena_extend_strategy(ep::ArenaExtendStrategy::SameAsRequested)
                            .with_tf32(false)
                            .build();
                        match build(options, Some(provider)) {
                            Ok(session) => {
                                return Ok(Loaded {
                                    session,
                                    provider: name,
                                    fallback_reasons: Vec::new(),
                                    cuda: Some(context),
                                });
                            }
                            Err(error) => failures.push(error.to_string()),
                        }
                    }
                    return Err(failures.join("; "));
                }
                "MIGraphX" if available(&ep::MIGraphX::default()) => {
                    ep::MIGraphX::default().with_mem_limit(2 * GIB).build()
                }
                "ROCm" if available(&ep::ROCm::default()) => {
                    ep::ROCm::default().with_mem_limit(2 * GIB).build()
                }
                "OpenVINO GPU" if available(&ep::OpenVINO::default()) => {
                    ep::OpenVINO::default().with_device_type("GPU").build()
                }
                "WebGPU" if available(&ep::WebGPU::default()) => ep::WebGPU::default().build(),
                _ => return Err("not included in the installed ONNX Runtime".into()),
            };
            build(options, Some(provider))
                .map(|session| Loaded {
                    session,
                    provider: name,
                    fallback_reasons: Vec::new(),
                    #[cfg(target_os = "linux")]
                    cuda: None,
                })
                .map_err(|e| e.to_string())
        },
        || cpu(options).map_err(|e| e.to_string()),
    )
    .map_err(io::Error::other)?;
    loaded.fallback_reasons = failures;
    Ok(loaded)
}

pub fn cpu(options: &Options) -> io::Result<Loaded> {
    // Recheck the full CPU budget before rebuilding after a GPU failure.
    e2em_runtime::runtime::resources::Resources::detect()?
        .model_plan(options.model_bytes, options.threads)?;
    Ok(Loaded {
        session: build(options, None).map_err(io::Error::other)?,
        provider: "CPU",
        fallback_reasons: Vec::new(),
        #[cfg(target_os = "linux")]
        cuda: None,
    })
}

pub fn options(
    model: &Path,
    model_bytes: u64,
    threads: usize,
    device: Device,
    profile: Option<PathBuf>,
) -> Options {
    Options {
        model: model.join("model.onnx"),
        model_bytes,
        threads,
        device,
        profile,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn auto_prefers_working_gpu_and_records_failed_attempts() {
        let (value, errors) = choose(
            Device::Auto,
            ["bad", "good"],
            |name| {
                if name == "bad" {
                    Err("driver unavailable".into())
                } else {
                    Ok("gpu")
                }
            },
            || panic!("CPU must not run when GPU works"),
        )
        .unwrap();
        assert_eq!(value, "gpu");
        assert_eq!(errors, ["bad: driver unavailable"]);
    }
    #[test]
    fn auto_falls_back_on_registration_or_model_load_failure() {
        let (value, errors) = choose(
            Device::Auto,
            ["gpu"],
            |_| Err("model initialization failed".into()),
            || Ok("cpu"),
        )
        .unwrap();
        assert_eq!(value, "cpu");
        assert_eq!(errors.len(), 1);
        assert_eq!(
            choose(Device::Auto, [], |_| panic!(), || Ok("cpu"))
                .unwrap()
                .0,
            "cpu"
        );
    }
    #[test]
    fn forced_gpu_refuses_cpu_and_forced_cpu_skips_gpu_probes() {
        assert!(
            choose::<()>(
                Device::Gpu,
                ["gpu"],
                |_| Err("out of VRAM".into()),
                || panic!("strict GPU must not fall back")
            )
            .is_err()
        );
        assert_eq!(
            choose(
                Device::Cpu,
                ["gpu"],
                |_| panic!("CPU mode must not probe GPUs"),
                || Ok("cpu")
            )
            .unwrap()
            .0,
            "cpu"
        );
    }
}
