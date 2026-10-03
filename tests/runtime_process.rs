#![cfg(feature = "runtime-probes")]
use e2em_runtime::{PolicyScorer, runtime::process::NativeProcessScorer};
use std::{
    ffi::OsString,
    path::Path,
    time::{Duration, Instant},
};
fn spawn(mode: &str, timeout: Duration) -> Result<NativeProcessScorer, e2em_runtime::BackendError> {
    let args: Vec<OsString> = ["--buffer-mib", "1", "--mode", mode]
        .into_iter()
        .map(Into::into)
        .collect();
    NativeProcessScorer::spawn(
        &std::fs::canonicalize(env!("CARGO_BIN_EXE_e2em-resource-worker")).unwrap(),
        &args,
        timeout,
    )
}
#[test]
fn native_worker_scores_and_drop_disposes_its_process() {
    let scorer = spawn("normal", Duration::from_secs(1)).unwrap();
    let pid = scorer.process_id().unwrap();
    assert_eq!(
        scorer
            .score("alex@example.test", None, "pii.email")
            .unwrap(),
        1.0
    );
    assert_eq!(scorer.score("hello", None, "pii.email").unwrap(), 0.0);
    // Valid maximum-size runtime targets, including JSON escape expansion,
    // must remain scoreable rather than hit an overly conservative estimate.
    assert_eq!(
        scorer.score(&"x".repeat(16384), None, "pii.email").unwrap(),
        0.0
    );
    assert_eq!(
        scorer
            .score(&"\0".repeat(16384), None, "pii.email")
            .unwrap(),
        0.0
    );
    assert!(
        scorer
            .score(&"\0".repeat(32768), None, "pii.email")
            .is_err()
    );
    assert!(
        scorer
            .score(&"x".repeat(131073), None, "pii.email")
            .is_err()
    );
    // A caller-side bounds error must not damage an otherwise healthy worker.
    assert_eq!(scorer.score("hello", None, "pii.email").unwrap(), 0.0);
    drop(scorer);
    #[cfg(target_os = "linux")]
    assert!(
        !Path::new(&format!("/proc/{pid}")).exists(),
        "eviction must reap the worker"
    );
    #[cfg(not(target_os = "linux"))]
    let _ = pid;
}
#[test]
fn hung_startup_and_incompatible_frames_fail_with_bounded_cleanup() {
    for mode in ["hang-startup", "wrong-version", "oversized-frame"] {
        let started = Instant::now();
        assert!(spawn(mode, Duration::from_millis(250)).is_err(), "{mode}");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "startup failure must not strand a reader thread"
        );
    }
}
#[test]
fn timeout_invalid_score_and_worker_exit_cannot_leave_a_live_worker_or_allow_result() {
    for mode in ["hang-score", "invalid-score", "exit-score"] {
        let scorer = spawn(mode, Duration::from_secs(1)).unwrap();
        let pid = scorer.process_id().unwrap();
        let started = Instant::now();
        assert!(
            scorer
                .score("alex@example.test", None, "pii.email")
                .is_err()
        );
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_eq!(scorer.process_id(), None);
        assert!(scorer.score("hello", None, "pii.email").is_err());
        #[cfg(target_os = "linux")]
        assert!(!Path::new(&format!("/proc/{pid}")).exists());
        #[cfg(not(target_os = "linux"))]
        let _ = pid;
    }
}
#[test]
fn executable_and_timeout_are_explicit_provisioning() {
    assert!(
        NativeProcessScorer::spawn(Path::new("relative-worker"), &[], Duration::from_secs(1))
            .is_err()
    );
    assert!(spawn("normal", Duration::ZERO).is_err());
    assert!(spawn("normal", Duration::from_secs(6)).is_err());
}

#[test]
fn native_worker_backend_failures_produce_review_through_the_runtime() {
    use e2em_runtime::runtime::{Action, Request, Status, scheduler::Scheduler};
    let scheduler = Scheduler::with_factory(Duration::from_secs(300), || {
        Ok(Box::new(spawn("invalid-score", Duration::from_secs(1))?))
    });
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("conformance/assessments.json")).unwrap();
    let request: Request = serde_json::from_value(cases[0]["request"].clone()).unwrap();
    let result = scheduler.submit("app", request).unwrap().wait().unwrap();
    assert_eq!(result.status, Status::Error);
    assert_eq!(result.action, Action::Review);
}
