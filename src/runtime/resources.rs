//! Admission checks for the CPU inference worker, without loading any model.
//! These conservative estimates are guards, not measured hardware qualification.
use std::{io, thread};
use sysinfo::{MemoryRefreshKind, System};

const MIB: u64 = 1024 * 1024;
// Allow graph/session allocations and working tensors as well as the OS. Swap
// is deliberately excluded: it is not an inference memory budget.
pub const INFERENCE_HEADROOM_BYTES: u64 = 1024 * MIB;
const MODEL_MEMORY_MULTIPLIER: u64 = 4;

#[derive(Clone, Copy, Debug)]
pub struct Resources {
    pub total_memory_bytes: u64,
    pub available_memory_bytes: u64,
    pub cpu_threads: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct Plan {
    pub threads: usize,
    pub required_available_memory_bytes: u64,
}

impl Resources {
    pub fn detect() -> io::Result<Self> {
        let cpu_threads = thread::available_parallelism()?.get();
        let mut system = System::new();
        system.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());
        let resources = Self {
            total_memory_bytes: system.total_memory(),
            available_memory_bytes: system.available_memory(),
            cpu_threads,
        };
        // System::cgroup_limits examines the root, which can miss a service's
        // narrower limit. Inspect this process and its cgroup ancestors instead.
        #[cfg(target_os = "linux")]
        let resources = {
            use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, get_current_pid};
            let mut resources = resources;
            let pid = get_current_pid().map_err(io::Error::other)?;
            system.refresh_processes_specifics(
                ProcessesToUpdate::Some(&[pid]),
                false,
                ProcessRefreshKind::nothing().without_tasks(),
            );
            let process = system
                .process(pid)
                .ok_or_else(|| io::Error::other("cannot inspect inference process resources"))?;
            if let Some(limits) = process.cgroup_limits() {
                resources.limit_memory(limits.total_memory, limits.free_memory);
            }
            resources
        };
        resources.validate()?;
        Ok(resources)
    }

    #[cfg(any(target_os = "linux", test))]
    fn limit_memory(&mut self, total: u64, available: u64) {
        self.total_memory_bytes = self.total_memory_bytes.min(total);
        self.available_memory_bytes = self.available_memory_bytes.min(available);
    }

    fn validate(self) -> io::Result<()> {
        if self.total_memory_bytes == 0 || self.cpu_threads == 0 {
            return Err(io::Error::other(
                "cannot determine inference CPU/RAM resources",
            ));
        }
        Ok(())
    }

    fn require_memory(self, required: u64) -> io::Result<()> {
        self.validate()?;
        let available = self.available_memory_bytes.min(self.total_memory_bytes);
        if available < required {
            return Err(io::Error::other(format!(
                "insufficient RAM for CPU inference: need {} MiB available, have {} MiB (swap excluded)",
                required.div_ceil(MIB),
                available / MIB,
            )));
        }
        Ok(())
    }

    pub fn model_plan(self, model_bytes: u64, requested_threads: usize) -> io::Result<Plan> {
        if model_bytes == 0 || requested_threads == 0 {
            return Err(io::Error::other("invalid inference resource requirements"));
        }
        let required = model_bytes
            .checked_mul(MODEL_MEMORY_MULTIPLIER)
            .and_then(|bytes| bytes.checked_add(INFERENCE_HEADROOM_BYTES))
            .ok_or_else(|| io::Error::other("inference memory estimate overflow"))?;
        self.require_memory(required)?;
        Ok(Plan {
            threads: requested_threads.min(self.cpu_threads).min(2),
            required_available_memory_bytes: required,
        })
    }

    /// Recheck headroom with the model already resident, before allocating a batch.
    pub fn admit_inference(self) -> io::Result<()> {
        self.require_memory(INFERENCE_HEADROOM_BYTES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> Resources {
        Resources {
            total_memory_bytes: 32 * 1024 * MIB,
            available_memory_bytes: 8 * 1024 * MIB,
            cpu_threads: 24,
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    #[test]
    fn native_snapshot_reads_resources_without_loading_a_model() {
        let resources = Resources::detect().unwrap();
        assert!(resources.total_memory_bytes > 0);
        assert!(resources.available_memory_bytes <= resources.total_memory_bytes);
        assert!(resources.cpu_threads > 0);
    }

    #[test]
    fn small_or_busy_machines_cannot_start_a_model() {
        let mut resources = host();
        resources.total_memory_bytes = 1024 * MIB;
        assert!(resources.model_plan(284_315_095, 2).is_err());
        resources = host();
        resources.available_memory_bytes = 1024 * MIB;
        let error = resources.model_plan(284_315_095, 2).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("insufficient RAM for CPU inference")
        );
    }

    #[test]
    fn service_memory_limits_override_large_host_resources() {
        let mut resources = host();
        resources.limit_memory(2 * 1024 * MIB, 512 * MIB);
        assert_eq!(resources.available_memory_bytes, 512 * MIB);
        assert!(resources.model_plan(284_315_095, 2).is_err());
        // An unlimited group must not hide low host availability.
        resources.limit_memory(u64::MAX, u64::MAX);
        assert_eq!(resources.available_memory_bytes, 512 * MIB);
    }

    #[test]
    fn thread_budget_obeys_single_cpu_allocations_and_runtime_cap() {
        let mut resources = host();
        assert_eq!(resources.model_plan(284_315_095, 16).unwrap().threads, 2);
        assert_eq!(resources.model_plan(284_315_095, 1).unwrap().threads, 1);
        resources.cpu_threads = 1;
        assert_eq!(resources.model_plan(284_315_095, 2).unwrap().threads, 1);
    }

    #[test]
    fn estimates_are_bounded_and_unknown_resources_refuse_inference() {
        assert!(host().model_plan(u64::MAX, 2).is_err());
        assert!(host().model_plan(0, 2).is_err());
        assert!(host().model_plan(1, 0).is_err());
        let mut resources = host();
        resources.total_memory_bytes = 0;
        assert!(resources.model_plan(1, 2).is_err());
        resources = host();
        resources.cpu_threads = 0;
        assert!(resources.model_plan(1, 2).is_err());
    }

    #[test]
    fn resident_model_requires_working_memory_and_can_recover() {
        let mut resources = host();
        let requirement = resources
            .model_plan(284_315_095, 2)
            .unwrap()
            .required_available_memory_bytes;
        resources.available_memory_bytes = requirement - 1;
        assert!(resources.model_plan(284_315_095, 2).is_err());
        resources.available_memory_bytes = requirement;
        assert!(resources.model_plan(284_315_095, 2).is_ok());
        resources.available_memory_bytes = INFERENCE_HEADROOM_BYTES - 1;
        assert!(resources.admit_inference().is_err());
        resources.available_memory_bytes = INFERENCE_HEADROOM_BYTES;
        assert!(resources.admit_inference().is_ok());
    }
}
