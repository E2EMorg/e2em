//! Linux CPU-buffer reclamation probe; synthetic assets, never model qualification.
#[cfg(target_os = "linux")]
mod linux {
    use clap::Parser;
    use e2em_runtime::{
        BackendError, PolicyScorer, RulesScorer,
        runtime::{
            Action, Policy, Request,
            process::NativeProcessScorer,
            scheduler::{Scheduler, State},
        },
    };
    use std::{
        fs, io,
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicU32, AtomicUsize, Ordering},
        },
        time::{Duration, Instant},
    };

    #[derive(Parser)]
    pub struct Args {
        #[arg(long)]
        output: PathBuf,
        #[arg(long, default_value_t = 3)]
        cycles: usize,
        #[arg(long, default_value_t = 64)]
        buffer_mib: usize,
        #[arg(long)]
        disposable_worker: Option<PathBuf>,
    }
    struct CpuBuffers {
        buffers: Vec<Vec<u8>>,
        scorer: RulesScorer,
        drops: Arc<AtomicUsize>,
    }
    impl PolicyScorer for CpuBuffers {
        fn score(
            &self,
            message: &str,
            context: Option<&str>,
            policy: &str,
        ) -> Result<f64, BackendError> {
            // Keep initialized pages observable through every scoring call.
            std::hint::black_box(&self.buffers);
            self.scorer.score(message, context, policy)
        }
    }
    impl Drop for CpuBuffers {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }
    struct DisposableBuffers {
        scorer: NativeProcessScorer,
        drops: Arc<AtomicUsize>,
    }
    impl PolicyScorer for DisposableBuffers {
        fn score(
            &self,
            message: &str,
            context: Option<&str>,
            policy: &str,
        ) -> Result<f64, BackendError> {
            self.scorer.score(message, context, policy)
        }
    }
    impl Drop for DisposableBuffers {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }
    fn rss_of(pid: u32) -> io::Result<usize> {
        fs::read_to_string(format!("/proc/{pid}/status"))?
            .lines()
            .find_map(|line| {
                line.strip_prefix("VmRSS:")
                    .and_then(|value| value.split_whitespace().next())
                    .and_then(|value| value.parse().ok())
            })
            .ok_or_else(|| io::Error::other("missing process RSS"))
    }
    fn rss() -> io::Result<usize> {
        rss_of(std::process::id())
    }
    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let args = Args::parse();
        if !(1..=20).contains(&args.cycles) || !(16..=512).contains(&args.buffer_mib) {
            return Err(io::Error::other("cycles must be 1–20 and CPU buffers 16–512 MiB").into());
        }
        let loads = Arc::new(AtomicUsize::new(0));
        let drops = Arc::new(AtomicUsize::new(0));
        let bytes = args.buffer_mib * 1024 * 1024;
        let child_pid = Arc::new(AtomicU32::new(0));
        let scheduler = Scheduler::with_factory(Duration::from_secs(1), {
            let loads = loads.clone();
            let drops = drops.clone();
            let child_pid = child_pid.clone();
            let worker = args.disposable_worker.clone();
            let buffer_mib = args.buffer_mib;
            move || {
                loads.fetch_add(1, Ordering::SeqCst);
                let backend: e2em_runtime::runtime::scheduler::Backend =
                    if let Some(worker) = &worker {
                        let scorer = NativeProcessScorer::spawn(
                            worker,
                            &["--buffer-mib".into(), buffer_mib.to_string().into()],
                            Duration::from_millis(500),
                        )?;
                        child_pid.store(scorer.process_id().unwrap(), Ordering::SeqCst);
                        Box::new(DisposableBuffers {
                            scorer,
                            drops: drops.clone(),
                        })
                    } else {
                        let buffers = (0..4).map(|_| vec![0xa5; bytes / 4]).collect();
                        Box::new(CpuBuffers {
                            buffers,
                            scorer: RulesScorer::new(),
                            drops: drops.clone(),
                        })
                    };
                Ok(backend)
            }
        });
        let policy: Policy =
            serde_json::from_str(include_str!("../tests/conformance/email-policy.json"))?;
        let reference = scheduler.validate_policy("resource-probe", policy)?;
        let cases: serde_json::Value =
            serde_json::from_str(include_str!("../tests/conformance/assessments.json"))?;
        let mut request: Request = serde_json::from_value(cases[0]["request"].clone())?;
        request.policy = None;
        request.policy_ref = Some(reference);
        let initial = rss()?;
        let mut cycles = Vec::new();
        for cycle in 0..args.cycles {
            let started = Instant::now();
            let jobs = (0..8)
                .map(|index| {
                    let mut request = request.clone();
                    request.request_id = format!("cycle-{cycle}-{index}");
                    scheduler.submit("resource-probe", request)
                })
                .collect::<Result<Vec<_>, _>>()?;
            for job in jobs {
                assert_eq!(job.wait()?.action, Action::Warn);
            }
            let coalesced_ms = started.elapsed().as_secs_f64() * 1000.;
            assert_eq!(loads.load(Ordering::SeqCst), cycle + 1);
            let broker_ready = rss()?;
            let pid = child_pid.load(Ordering::SeqCst);
            let worker_ready = if pid != 0 { rss_of(pid)? } else { 0 };
            let ready = broker_ready + worker_ready;
            assert!(
                ready >= initial + args.buffer_mib * 1024 / 2,
                "page-touched CPU buffers must be resident"
            );
            let cause = if cycle % 2 == 0 {
                "idle timeout"
            } else {
                "explicit memory pressure"
            };
            if cycle % 2 != 0 {
                assert!(scheduler.memory_pressure());
            }
            let expires = Instant::now() + Duration::from_secs(5);
            while scheduler.state() != State::Unloaded {
                assert!(
                    Instant::now() < expires,
                    "runtime failed to unload CPU assets"
                );
                std::thread::sleep(Duration::from_millis(2));
            }
            assert_eq!(drops.load(Ordering::SeqCst), cycle + 1);
            let unloaded = rss()?;
            cycles.push(serde_json::json!({"cause":cause,"ready_rss_kib":ready,"unloaded_rss_kib":unloaded,
                "broker_ready_rss_kib":broker_ready,"worker_ready_rss_kib":worker_ready,
                "worker_reaped":pid != 0 && !std::path::Path::new(&format!("/proc/{pid}")).exists(),
                "returned_to_baseline":unloaded <= initial + 8 * 1024,
                "released_rss_kib":ready.saturating_sub(unloaded),"coalesced_eight_request_duration_ms":coalesced_ms,
                "loads":loads.load(Ordering::SeqCst),"drops":drops.load(Ordering::SeqCst)}));
        }
        let report = serde_json::json!({"platform":"Linux native Rust CPU allocation probe", "assets":"synthetic page-touched CPU buffers",
            "mode":if args.disposable_worker.is_some() { "disposable native worker" } else { "in-process scorer" },
            "allocated_mib":args.buffer_mib,"initial_unloaded_rss_kib":initial,"cycles":cycles,
            "model":null,"accelerator":null,"limits":"Tests owned CPU allocation disposal through the shared runtime loader, including coalesced reload and policy retention. No tokenizer/model inference, accelerator or model-quality qualification."});
        fs::write(args.output, serde_json::to_string_pretty(&report)? + "\n")?;
        if report["cycles"]
            .as_array()
            .unwrap()
            .iter()
            .any(|cycle| cycle["returned_to_baseline"] != true)
        {
            return Err(io::Error::other(
                "allocator retained CPU memory; use a disposable worker (report saved)",
            )
            .into());
        }
        Ok(())
    }
}
#[cfg(target_os = "linux")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    linux::run()
}
#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("CPU resource probe requires native Linux /proc RSS counters");
}
