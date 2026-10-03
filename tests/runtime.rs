use e2em_runtime::{
    BackendError, PolicyScorer,
    runtime::{
        scheduler::{Scheduler, State},
        *,
    },
};
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
fn policy() -> Policy {
    serde_json::from_str(include_str!("conformance/email-policy.json")).unwrap()
}
fn request() -> Request {
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("conformance/assessments.json")).unwrap();
    serde_json::from_value(cases[0]["request"].clone()).unwrap()
}
#[test]
fn golden_assessments_preserve_original_bytes() {
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("conformance/assessments.json")).unwrap();
    let engine = Engine::default();
    for case in cases.as_array().unwrap() {
        let request: Request = serde_json::from_value(case["request"].clone()).unwrap();
        let result = engine.assess("app", &request);
        let encoded = serde_json::to_value(&result).unwrap();
        assert_eq!(
            encoded["status"], case["expected"]["status"],
            "{}",
            case["name"]
        );
        assert_eq!(
            encoded["action"], case["expected"]["action"],
            "{}",
            case["name"]
        );
        let spans: Vec<_> = result
            .findings
            .iter()
            .flat_map(|f| &f.spans)
            .map(|s| {
                assert!(request.message.text.is_char_boundary(s.start));
                assert!(request.message.text.is_char_boundary(s.end));
                serde_json::json!([s.start, s.end])
            })
            .collect();
        assert_eq!(
            serde_json::json!(spans),
            case["expected"]["spans"],
            "{}",
            case["name"]
        );
        assert!(
            !serde_json::to_string(&result)
                .unwrap()
                .contains("alex@example")
        );
    }
}
#[test]
fn rejects_unknown_fields_and_invalid_request_bounds() {
    let mut value = serde_json::to_value(request()).unwrap();
    value["unexpected"] = true.into();
    assert!(serde_json::from_value::<Request>(value).is_err());
    let engine = Engine::default();
    for index in 0..8 {
        let mut r = request();
        match index {
            0 => r.message.text.clear(),
            1 => r.message.text = "a".repeat(16385),
            2 => r.request_id.clear(),
            3 => r.options.deadline_ms = 5001,
            4 => r.api_version = "2".into(),
            5 => r.context.push(Turn {
                id: r.message.id.clone(),
                speaker: "peer".into(),
                text: "x".into(),
            }),
            6 => {
                r.policy_ref = Some(PolicyRef {
                    provider: "p".into(),
                    id: "p".into(),
                    version: "1".into(),
                    token: "t".into(),
                })
            }
            _ => {
                r.policy = None;
            }
        }
        let result = engine.assess("app", &r);
        assert_eq!(result.status, Status::Error);
        assert_eq!(result.action, Action::Review);
        assert!(result.error_code.is_some());
    }
}
#[test]
fn policy_references_are_immutable_scoped_and_revocable() {
    let engine = Engine::default();
    let reference = engine.validate_policy("a", policy()).unwrap();
    let mut r = request();
    r.policy = None;
    r.policy_ref = Some(reference);
    assert_eq!(engine.assess("a", &r).status, Status::Assessed);
    assert_eq!(
        engine.assess("b", &r).error_code,
        Some(ErrorCode::PolicyNotFound)
    );
    assert_eq!(
        Engine::default().assess("a", &r).error_code,
        Some(ErrorCode::PolicyNotFound)
    );
    let mut p = policy();
    p.rules[0].action = Action::Review;
    assert_eq!(
        engine.validate_policy("a", p),
        Err(ErrorCode::InvalidPolicy)
    );
    engine.revoke("a");
    assert_eq!(
        engine.assess("a", &r).error_code,
        Some(ErrorCode::PolicyNotFound)
    );
}
#[test]
fn missing_context_and_stale_revisions_fail_safely() {
    let engine = Engine::default();
    let mut r = request();
    let rule = &mut r.policy.as_mut().unwrap().rules[0];
    rule.context_requirement = ContextRequirement::SuppliedWindow;
    rule.min_context_messages = Some(2);
    let result = engine.assess("a", &r);
    assert_eq!(result.status, Status::Indeterminate);
    assert_eq!(result.action, Action::Review);
    assert_eq!(result.reason_codes, ["INSUFFICIENT_CONTEXT"]);
    assert!(result.applies_to(&r));
    r.message.revision = "4".into();
    assert!(!result.applies_to(&r));
}
struct Stub {
    calls: AtomicUsize,
    answer: f64,
}
impl PolicyScorer for Stub {
    fn score(&self, _: &str, _: Option<&str>, _: &str) -> Result<f64, BackendError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(self.answer)
    }
}
#[test]
fn unavailable_detectors_are_reported_and_invalid_scores_review() {
    for answer in [f64::NAN, f64::INFINITY, -0.1, 0.5, 2.0] {
        let engine = Engine::new(Stub {
            calls: AtomicUsize::new(0),
            answer,
        });
        let mut r = request();
        r.policy.as_mut().unwrap().rules[0].category = Some("abuse.threat".into());
        let result = engine.assess("a", &r);
        assert_eq!(result.status, Status::Indeterminate);
        assert_eq!(result.error_code, None);
        assert_eq!(result.reason_codes, ["DETECTOR_UNAVAILABLE"]);
        r.policy = Some(policy());
        assert_eq!(
            engine.assess("a", &r).error_code,
            Some(ErrorCode::InternalError)
        );
    }
}
#[test]
fn custom_policies_are_accepted_but_blocking_remains_invalid() {
    let mut p = policy();
    let rule = &mut p.rules[0];
    rule.category = None;
    rule.policy_text = Some("a custom rule".into());
    rule.r#match = Match::PolicyText;
    assert_eq!(validate_policy(&p), Ok(()));
    p.rules[0].action = Action::Block;
    assert_eq!(validate_policy(&p), Err(ErrorCode::InvalidPolicy));
    let mut p = policy();
    p.rules[0].r#match = Match::Score;
    p.rules[0].review_threshold = Some(0.4);
    p.rules[0].action_threshold = Some(0.7);
    p.rules[0].action = Action::Block;
    assert_eq!(validate_policy(&p), Err(ErrorCode::InvalidPolicy));
}

#[test]
fn all_valid_policy_categories_and_custom_text_can_be_reported() {
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("conformance/policy-reports.json")).unwrap();
    let engine = Engine::default();
    for case in cases.as_array().unwrap() {
        let mut r = request();
        let policy: Policy = serde_json::from_value(case["policy"].clone()).unwrap();
        r.policy = Some(policy.clone());
        let inline = engine.assess("app", &r);
        let mut policy = policy;
        policy.id = case["name"].as_str().unwrap().into();
        r.policy_ref = Some(engine.validate_policy("app", policy).unwrap());
        r.policy = None;
        let referenced = engine.assess("app", &r);
        for result in [inline, referenced] {
            let value = serde_json::to_value(result).unwrap();
            assert_eq!(value["status"], case["expected"]["status"]);
            assert_eq!(value["action"], case["expected"]["action"]);
            assert_eq!(value["coverage"]["unevaluated_rules"], case["expected"]["unevaluated_rules"]);
            assert_eq!(value["reason_codes"], case["expected"]["reason_codes"]);
            assert!(value["findings"].as_array().unwrap().is_empty());
        }
    }
}

struct ModelStub {
    answer: f64,
}
impl PolicyScorer for ModelStub {
    fn supports_model_policies(&self) -> bool {
        true
    }
    fn model_version(&self) -> String {
        "experimental-test-model".into()
    }
    fn score(&self, message: &str, context: Option<&str>, policy: &str) -> Result<f64, BackendError> {
        assert_eq!(message, "🙂 Original chat text");
        assert_eq!(context, Some("Previous chat message"));
        assert!(matches!(policy, "experimental.category" | "Do not discuss a former partner."));
        Ok(self.answer)
    }
    fn action_threshold(&self, _: &str) -> Option<f64> {
        Some(0.7)
    }
}

fn model_request(custom: bool) -> Request {
    let mut r = request();
    r.message.text = "🙂 Original chat text".into();
    r.context = vec![Turn {
        id: "previous".into(), speaker: "peer".into(), text: "Previous chat message".into(),
    }];
    let rule = &mut r.policy.as_mut().unwrap().rules[0];
    rule.context_requirement = ContextRequirement::SuppliedWindow;
    rule.min_context_messages = Some(1);
    if custom {
        rule.r#match = Match::PolicyText;
        rule.category = None;
        rule.policy_text = Some("Do not discuss a former partner.".into());
    } else {
        rule.r#match = Match::Score;
        rule.category = Some("experimental.category".into());
        rule.review_threshold = Some(0.4);
        rule.action_threshold = Some(0.7);
    }
    r
}

#[test]
fn arbitrary_model_policies_report_every_score_without_a_rating_gate() {
    for custom in [false, true] {
        for score in [0.0, 0.2, 0.5, 0.7, 1.0] {
            let engine = Engine::new(ModelStub { answer: score });
            let r = model_request(custom);
            let result = engine.assess("app", &r);
            assert_eq!(result.status, Status::Assessed);
            assert_eq!(result.versions.model, "experimental-test-model");
            assert_eq!(result.findings.len(), 1);
            assert_eq!(result.findings[0].score, Some(score));
            assert_eq!(result.findings[0].method, if custom { Method::CustomPolicy } else { Method::Model });
            assert!(result.findings[0].spans.is_empty());
            assert_eq!(result.action, if score >= 0.7 { Action::Warn } else if !custom && score >= 0.4 { Action::Review } else { Action::Allow });
            assert!(result.coverage.unevaluated_rules.is_empty());
        }
    }
}

#[test]
fn invalid_model_outputs_review_and_missing_context_does_not_score() {
    for score in [f64::NAN, f64::INFINITY, -0.1, 2.0] {
        let result = Engine::new(ModelStub { answer: score }).assess("app", &model_request(false));
        assert_eq!(result.error_code, Some(ErrorCode::InternalError));
        assert_eq!(result.action, Action::Review);
        assert_eq!(result.coverage.unevaluated_rules, ["email-warning"]);
    }
    let mut r = model_request(true);
    r.context.clear();
    let result = Engine::new(ModelStub { answer: 0.7 }).assess("app", &r);
    assert_eq!(result.status, Status::Indeterminate);
    assert_eq!(result.reason_codes, ["INSUFFICIENT_CONTEXT"]);
}

#[test]
fn scheduler_forwards_model_support_and_custom_thresholds() {
    let scheduler = Scheduler::with_factory(Duration::from_secs(300), || {
        Ok(Box::new(ModelStub { answer: 0.6 }))
    });
    let result = scheduler.submit("app", model_request(true)).unwrap().wait().unwrap();
    assert_eq!(result.status, Status::Assessed);
    assert_eq!(result.action, Action::Allow);
    assert_eq!(result.findings[0].score, Some(0.6));
    assert_eq!(scheduler.capabilities().model, "experimental-test-model");
    assert_eq!(scheduler.capabilities().custom_policies, "supported");
}

#[test]
fn unavailable_rules_do_not_hide_completed_reports_or_allow_the_message() {
    let mut r = request();
    let mut unavailable = r.policy.as_ref().unwrap().rules[0].clone();
    unavailable.id = "unavailable".into();
    unavailable.category = Some("experimental.category".into());
    r.policy.as_mut().unwrap().rules.push(unavailable);
    let result = Engine::default().assess("app", &r);
    assert_eq!(result.status, Status::Indeterminate);
    assert_eq!(result.action, Action::Review);
    assert_eq!(result.findings.len(), 1);
    assert_eq!(result.findings[0].rule_id, "email-warning");
    assert_eq!(result.coverage.unevaluated_rules, ["unavailable"]);
}
#[test]
fn rules_aggregate_and_only_apply_in_requested_direction() {
    let mut r = request();
    let mut second = r.policy.as_ref().unwrap().rules[0].clone();
    second.id = "review-email".into();
    second.action = Action::Review;
    r.policy.as_mut().unwrap().rules.push(second);
    assert_eq!(Engine::default().assess("a", &r).action, Action::Review);
    r.direction = Direction::Incoming;
    assert_eq!(Engine::default().assess("a", &r).action, Action::Allow);
}
#[test]
fn scheduler_completion_cancellation_and_idle_eviction() {
    let scheduler = Scheduler::new(Duration::from_millis(10));
    let r = request();
    assert!(!scheduler.cancel("other", &r.request_id));
    let pending = scheduler.submit("a", r.clone()).unwrap();
    assert!(!scheduler.cancel("b", &r.request_id));
    scheduler.cancel("a", &r.request_id);
    let result = pending.wait().unwrap();
    assert!(matches!(
        result.status,
        Status::Assessed | Status::Cancelled
    ));
    for _ in 0..100 {
        if scheduler.state() == State::Unloaded {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(scheduler.state(), State::Unloaded);
    // Capabilities/connected idle observers cannot extend the timer.
    assert!(scheduler.capabilities().backend_ready);
    assert_eq!(scheduler.state(), State::Unloaded);
    assert_eq!(
        scheduler.submit("a", r).unwrap().wait().unwrap().action,
        Action::Warn
    );
    assert!(scheduler.memory_pressure());
    assert_eq!(scheduler.state(), State::Unloaded);
}
#[test]
fn discovery_uses_verified_compatible_native_provider_without_downgrade() {
    use e2em_runtime::runtime::discovery::*;
    let capabilities = Engine::default().capabilities();
    let mut project = Candidate {
        kind: ProviderKind::Project,
        authenticated: true,
        authorised: true,
        capabilities: capabilities.clone(),
    };
    project.capabilities.provider = "project".into();
    let mut native = Candidate {
        kind: ProviderKind::Native,
        authenticated: true,
        authorised: true,
        capabilities,
    };
    native.capabilities.provider = "native".into();
    let providers = [project.clone(), native.clone()];
    let required = ["pii.email".into()];
    assert_eq!(
        select(&providers, None, &required)
            .unwrap()
            .capabilities
            .provider,
        "native"
    );
    assert_eq!(
        select(&providers, Some("project"), &required)
            .unwrap()
            .capabilities
            .provider,
        "project"
    );
    native.authenticated = false;
    project.capabilities.detectors.clear();
    assert!(select(&[native, project], None, &required).is_err());
}

#[test]
fn fair_bounded_queues_deadlines_revocation_and_cancellation_are_deterministic() {
    use std::sync::{Arc, Condvar, Mutex, mpsc};
    struct Gate {
        started: mpsc::Sender<()>,
        gate: Arc<(Mutex<bool>, Condvar)>,
        calls: Arc<Mutex<Vec<String>>>,
    }
    impl PolicyScorer for Gate {
        fn score(&self, message: &str, _: Option<&str>, _: &str) -> Result<f64, BackendError> {
            self.calls.lock().unwrap().push(message.into());
            if message.starts_with("hold") {
                self.started.send(()).unwrap();
                let (lock, changed) = &*self.gate;
                let mut open = lock.lock().unwrap();
                while !*open {
                    open = changed.wait(open).unwrap();
                }
            }
            Ok(1.0)
        }
    }
    let (started, entered) = mpsc::channel();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let scheduler = Scheduler::with_factory(Duration::from_secs(300), {
        let gate = gate.clone();
        let calls = calls.clone();
        move || {
            Ok(Box::new(Gate {
                started: started.clone(),
                gate: gate.clone(),
                calls: calls.clone(),
            }))
        }
    });
    let mut held = request();
    held.request_id = "hold".into();
    held.message.text = "hold alex@example.test".into();
    let first = scheduler.submit("a", held).unwrap();
    entered.recv_timeout(Duration::from_secs(1)).unwrap();
    let mut queued = Vec::new();
    for i in 0..7 {
        let mut r = request();
        r.request_id = format!("a-{i}");
        r.message.text = format!("a-{i} alex@example.test");
        if i == 0 {
            r.options.deadline_ms = 1;
        }
        queued.push(scheduler.submit("a", r).unwrap());
    }
    assert!(matches!(
        scheduler.submit("a", request()),
        Err(ErrorCode::ResourceExhausted)
    ));
    assert!(!scheduler.cancel("b", "a-1"));
    assert!(scheduler.cancel("a", "a-1"));
    let mut other = request();
    other.request_id = "b".into();
    other.message.text = "b alex@example.test".into();
    let second = scheduler.submit("b", other).unwrap();
    assert!(!scheduler.memory_pressure());
    assert!(matches!(
        scheduler.submit("new", request()),
        Err(ErrorCode::ResourceExhausted)
    ));
    // Allow the queued deadline to expire while inference owns its resources.
    std::thread::sleep(Duration::from_millis(5));
    *gate.0.lock().unwrap() = true;
    gate.1.notify_one();
    assert_eq!(first.wait().unwrap().status, Status::Assessed);
    assert_eq!(
        queued.remove(0).wait().unwrap().error_code,
        Some(ErrorCode::DeadlineExceeded)
    );
    assert_eq!(queued.remove(0).wait().unwrap().status, Status::Cancelled);
    assert_eq!(second.wait().unwrap().status, Status::Assessed);
    for pending in queued {
        assert_eq!(pending.wait().unwrap().status, Status::Assessed);
    }
    let calls = calls.lock().unwrap();
    assert_eq!(calls[1], "b alex@example.test");
    assert!(
        !calls
            .iter()
            .any(|c| c.starts_with("a-0") || c.starts_with("a-1"))
    );
}

#[test]
fn cold_load_coalesces_idle_eviction_releases_resources_and_reload_preserves_policy() {
    use std::sync::Arc;
    struct Resource {
        drops: Arc<AtomicUsize>,
        _buffer: Vec<u8>,
    }
    impl Drop for Resource {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }
    impl PolicyScorer for Resource {
        fn score(&self, _: &str, _: Option<&str>, _: &str) -> Result<f64, BackendError> {
            Ok(1.0)
        }
    }
    let loads = Arc::new(AtomicUsize::new(0));
    let drops = Arc::new(AtomicUsize::new(0));
    let scheduler = Scheduler::with_factory(Duration::from_millis(10), {
        let loads = loads.clone();
        let drops = drops.clone();
        move || {
            loads.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(Resource {
                drops: drops.clone(),
                _buffer: vec![1; 1024 * 1024],
            }))
        }
    });
    let reference = scheduler.validate_policy("a", policy()).unwrap();
    let mut r = request();
    r.policy = None;
    r.policy_ref = Some(reference);
    let jobs: Vec<_> = (0..8)
        .map(|i| {
            let mut r = r.clone();
            r.request_id = format!("req-{i}");
            scheduler.submit("a", r).unwrap()
        })
        .collect();
    for job in jobs {
        assert_eq!(job.wait().unwrap().action, Action::Warn);
    }
    assert_eq!(loads.load(Ordering::SeqCst), 1);
    for _ in 0..100 {
        if drops.load(Ordering::SeqCst) == 1 {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert_eq!(scheduler.state(), State::Unloaded);
    assert_eq!(
        scheduler.submit("a", r).unwrap().wait().unwrap().action,
        Action::Warn
    );
    assert_eq!(loads.load(Ordering::SeqCst), 2);
    assert!(scheduler.memory_pressure());
    assert_eq!(drops.load(Ordering::SeqCst), 2);
}
#[test]
fn cold_loading_deadline_expires_before_inference_and_worker_panics_never_allow() {
    struct NeverScore;
    impl PolicyScorer for NeverScore {
        fn score(&self, _: &str, _: Option<&str>, _: &str) -> Result<f64, BackendError> {
            panic!("must not score an expired cold request")
        }
    }
    let calls = std::sync::Arc::new(AtomicUsize::new(0));
    let (entered, loading) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let released = std::sync::Mutex::new(released);
    let scheduler = Scheduler::with_factory(Duration::from_secs(300), {
        let calls = calls.clone();
        move || {
            calls.fetch_add(1, Ordering::SeqCst);
            entered.send(()).unwrap();
            released
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            Ok(Box::new(NeverScore))
        }
    });
    let mut r = request();
    r.options.deadline_ms = 1000;
    let expires = std::time::Instant::now() + Duration::from_millis(r.options.deadline_ms);
    let pending = scheduler.submit("a", r).unwrap();
    // Prove the request expires during loading, rather than racing a one-ms
    // admission deadline against the OS scheduling the worker for the first time.
    loading.recv_timeout(Duration::from_secs(2)).unwrap();
    std::thread::sleep(
        expires.saturating_duration_since(std::time::Instant::now()) + Duration::from_millis(5),
    );
    release.send(()).unwrap();
    let result = pending.wait().unwrap();
    assert_eq!(result.error_code, Some(ErrorCode::DeadlineExceeded));
    assert_eq!(result.action, Action::Review);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let result = scheduler.submit("a", request()).unwrap().wait().unwrap();
    assert_eq!(result.action, Action::Review);
    assert_eq!(result.error_code, Some(ErrorCode::InternalError));
}
#[cfg(feature = "runtime-service")]
#[tokio::test]
async fn rust_async_sdk_awaits_completion() {
    let scheduler = Scheduler::default();
    let result = scheduler.submit("a", request()).unwrap().await.unwrap();
    assert_eq!(result.action, Action::Warn);
}

#[test]
fn shared_policy_failures_match_contract() {
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("conformance/policy-failures.json")).unwrap();
    for case in cases.as_array().unwrap() {
        let expected: ErrorCode = serde_json::from_value(case["error_code"].clone()).unwrap();
        let frame = serde_json::to_vec(&serde_json::json!({"call_id":"c","api_version":"0.1","operation":{"op":"validate_policy","policy":case["policy"]}})).unwrap();
        let result = match decode_call(&frame) {
            Ok(Call {
                operation: Operation::ValidatePolicy { policy },
                ..
            }) => validate_policy(&policy),
            Err(error) => Err(error.error_code),
            _ => panic!("expected policy call"),
        };
        assert_eq!(result, Err(expected), "{}", case["name"]);
    }
}
#[test]
fn request_options_have_individual_defaults() {
    let mut value = serde_json::to_value(request()).unwrap();
    value["options"] = serde_json::json!({"include_spans":true});
    let request: Request = serde_json::from_value(value).unwrap();
    assert_eq!(request.options.deadline_ms, 1000);
    assert!(request.options.include_spans);
}

#[test]
fn send_confirmation_cannot_apply_after_text_context_or_policy_edits() {
    use e2em_runtime::runtime::integration::RevisionGuard;
    let r = request();
    let guard = RevisionGuard::new(r.clone());
    let result = Engine::default().assess("app", &r);
    assert!(!guard.can_continue(&result, &r, false));
    assert!(guard.can_continue(&result, &r, true));
    for index in 0..4 {
        let mut edited = r.clone();
        match index {
            0 => edited.message.text.push('!'),
            1 => edited.context.push(Turn {
                id: "context".into(),
                speaker: "peer".into(),
                text: "hi".into(),
            }),
            2 => edited.policy.as_mut().unwrap().version = "2".into(),
            _ => edited.message.revision = "4".into(),
        }
        assert!(!guard.can_continue(&result, &edited, true));
    }
    let mut error = result.clone();
    error.status = Status::Error;
    error.action = Action::Review;
    assert!(!guard.can_continue(&error, &r, true));
    let mut forbidden = r.clone();
    forbidden.policy.as_mut().unwrap().r#override = Override::Forbidden;
    let result = Engine::default().assess("app", &forbidden);
    assert!(!RevisionGuard::new(forbidden.clone()).can_continue(&result, &forbidden, true));
}

#[test]
fn oversized_span_results_fail_without_unbounded_response_growth() {
    let mut r = request();
    r.message.text = "a@b.co ".repeat(600);
    let result = Engine::default().assess("a", &r);
    assert_eq!(result.error_code, Some(ErrorCode::ResourceExhausted));
    assert_eq!(result.action, Action::Review);
    assert!(serde_json::to_vec(&result).unwrap().len() < MAX_FRAME);
    r.options.include_spans = false;
    assert_eq!(Engine::default().assess("a", &r).action, Action::Warn);
}
#[test]
fn failed_loader_is_not_silently_retried() {
    let calls = std::sync::Arc::new(AtomicUsize::new(0));
    let scheduler = Scheduler::with_factory(Duration::from_millis(1), {
        let calls = calls.clone();
        move || {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(BackendError::new("synthetic load failure"))
        }
    });
    let result = scheduler.submit("a", request()).unwrap().wait().unwrap();
    assert_eq!(result.error_code, Some(ErrorCode::ModelUnavailable));
    assert!(matches!(
        scheduler.submit("a", request()),
        Err(ErrorCode::ModelUnavailable)
    ));
    assert_eq!(scheduler.state(), State::Failed);
    assert!(!scheduler.capabilities().backend_ready);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(scheduler.memory_pressure());
    assert_eq!(scheduler.state(), State::Failed);
}
