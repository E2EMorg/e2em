//! Rules-first embedded runtime, distinct from the research audit API.
pub mod contract;
pub mod discovery;
pub mod integration;
pub mod process;
pub mod scheduler;
#[cfg(feature = "runtime-service")]
pub mod service;

use crate::{PolicyScorer, RulesScorer};
pub use contract::*;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};
use unicode_normalization::UnicodeNormalization;

static INSTANCE: AtomicU64 = AtomicU64::new(1);

pub struct Engine<S = RulesScorer> {
    scorer: S,
    provider: String,
    policies: Mutex<HashMap<(String, String, String), Policy>>,
}
impl Default for Engine {
    fn default() -> Self {
        Self::new(RulesScorer::new())
    }
}
impl<S: PolicyScorer> Engine<S> {
    pub fn new(scorer: S) -> Self {
        let provider = format!(
            "embedded-{}-{}",
            std::process::id(),
            INSTANCE.fetch_add(1, Ordering::Relaxed)
        );
        Self {
            scorer,
            provider,
            policies: Mutex::new(HashMap::new()),
        }
    }
    pub fn capabilities(&self) -> Capabilities {
        Capabilities {
            api_version: API_VERSION.into(),
            provider: self.provider.clone(),
            runtime: env!("CARGO_PKG_VERSION").into(),
            model: self.scorer.model_version(),
            tokenizer: "none".into(),
            registry: "0.1.0".into(),
            languages: vec!["en".into(), "und".into()],
            detectors: vec!["pii.email".into()],
            presets: vec![],
            custom_policies: if self.scorer.supports_model_policies() {
                "supported"
            } else {
                "report-only"
            }
            .into(),
            profiles: vec![Profile::Personal],
            actions: vec![Action::Warn, Action::Review],
            limits: Limits {
                message_bytes: 16_384,
                request_text_bytes: 65_536,
                context_turns: 32,
                policy_rules: 64,
                frame_bytes: MAX_FRAME,
                result_spans: 512,
                deadline_ms: 5000,
            },
            backend_ready: true,
            runtime_state: scheduler::State::Ready,
            max_tokens: None,
        }
    }
    /// The trusted adapter supplies `principal`; display names confer no authority.
    pub fn validate_policy(&self, principal: &str, policy: Policy) -> Result<PolicyRef, ErrorCode> {
        validate_policy(&policy)?;
        if !identifier(principal) {
            return Err(ErrorCode::InvalidRequest);
        }
        let encoded = serde_json::to_vec(&policy).map_err(|_| ErrorCode::InvalidPolicy)?;
        let token = format!("{:x}", Sha256::digest(encoded));
        let key = (principal.into(), policy.id.clone(), policy.version.clone());
        let mut policies = self.policies.lock().map_err(|_| ErrorCode::InternalError)?;
        if let Some(old) = policies.get(&key) {
            if old != &policy {
                return Err(ErrorCode::InvalidPolicy);
            }
        } else {
            if policies
                .keys()
                .filter(|(owner, _, _)| owner == principal)
                .count()
                >= 64
            {
                return Err(ErrorCode::ResourceExhausted);
            }
            policies.insert(key, policy.clone());
        }
        Ok(PolicyRef {
            provider: self.provider.clone(),
            id: policy.id,
            version: policy.version,
            token,
        })
    }
    pub fn revoke(&self, principal: &str) {
        if let Ok(mut policies) = self.policies.lock() {
            policies.retain(|(owner, _, _), _| owner != principal);
        }
    }
    fn resolve(&self, principal: &str, request: &Request) -> Result<Policy, ErrorCode> {
        match (&request.policy_ref, &request.policy) {
            (Some(reference), None) => {
                if reference.provider != self.provider {
                    return Err(ErrorCode::PolicyNotFound);
                }
                let policies = self.policies.lock().map_err(|_| ErrorCode::InternalError)?;
                let policy = policies
                    .get(&(
                        principal.into(),
                        reference.id.clone(),
                        reference.version.clone(),
                    ))
                    .ok_or(ErrorCode::PolicyNotFound)?;
                let token = format!(
                    "{:x}",
                    Sha256::digest(
                        serde_json::to_vec(policy).map_err(|_| ErrorCode::InternalError)?
                    )
                );
                if token != reference.token {
                    return Err(ErrorCode::PolicyNotFound);
                }
                Ok(policy.clone())
            }
            (None, Some(policy)) => {
                validate_policy(policy)?;
                Ok(policy.clone())
            }
            _ => Err(ErrorCode::InvalidRequest),
        }
    }
    /// Blocking primitive for embedded bindings. Async clients use Scheduler.
    pub fn assess(&self, principal: &str, request: &Request) -> Assessment {
        let started = Instant::now();
        let mut result = base_result(request);
        result.versions.model = self.scorer.model_version();
        let execute = || -> Result<Policy, ErrorCode> {
            validate_request(request)?;
            if !identifier(principal) {
                return Err(ErrorCode::InvalidRequest);
            }
            self.resolve(principal, request)
        };
        let policy = match execute() {
            Ok(policy) => policy,
            Err(code) => {
                fail(&mut result, code);
                return result;
            }
        };
        result.versions.policy_id = policy.id.clone();
        result.versions.policy_version = policy.version.clone();
        result.coverage.target_complete = true;
        result.coverage.context_complete = true;
        let (normal, mapping) = normalize(&request.message.text);
        let applicable: Vec<_> = policy
            .rules
            .iter()
            .filter(|r| r.directions.contains(&request.direction))
            .collect();
        if request.language != "en" && request.language != "auto" && request.language != "und" {
            result.status = Status::Indeterminate;
            result.reason_codes.push("UNSUPPORTED_LANGUAGE".into());
            result.coverage.unevaluated_rules = applicable.iter().map(|r| r.id.clone()).collect();
            return result;
        }
        result.coverage.language = if request.language == "auto" {
            "und".into()
        } else {
            request.language.clone()
        };
        result.status = Status::Assessed;
        result.action = Action::Allow;
        for (index, rule) in applicable.iter().enumerate() {
            if rule.context_requirement == ContextRequirement::SuppliedWindow
                && request.context.len() < rule.min_context_messages.unwrap_or(1)
            {
                result.status = Status::Indeterminate;
                result.coverage.unevaluated_rules.push(rule.id.clone());
                result.reason_codes.push("INSUFFICIENT_CONTEXT".into());
                continue;
            }
            if rule.r#match != Match::Detected {
                if !self.scorer.supports_model_policies() {
                    result.status = Status::Indeterminate;
                    result.coverage.unevaluated_rules.push(rule.id.clone());
                    result.reason_codes.push("MODEL_UNAVAILABLE".into());
                    continue;
                }
                let policy_text = match rule.r#match {
                    Match::Score => rule.category.as_deref().expect("validated category"),
                    Match::PolicyText => rule.policy_text.as_deref().expect("validated policy text"),
                    Match::Detected => unreachable!(),
                };
                let context = (rule.context_requirement == ContextRequirement::SuppliedWindow)
                    .then(|| {
                        request
                            .context
                            .iter()
                            .map(|turn| turn.text.as_str())
                            .collect::<Vec<_>>()
                            .join("\n")
                    });
                let score = match self.scorer.score(
                    &request.message.text,
                    context.as_deref(),
                    policy_text,
                ) {
                    Ok(score) if score.is_finite() && (0.0..=1.0).contains(&score) => score,
                    _ => {
                        result
                            .coverage
                            .unevaluated_rules
                            .extend(applicable[index..].iter().map(|r| r.id.clone()));
                        fail(&mut result, ErrorCode::InternalError);
                        break;
                    }
                };
                let threshold = match rule.r#match {
                    Match::Score => rule.action_threshold.expect("validated threshold"),
                    Match::PolicyText => self.scorer.action_threshold(policy_text).unwrap_or(0.5),
                    Match::Detected => unreachable!(),
                };
                if !threshold.is_finite() || !(0.0..=1.0).contains(&threshold) {
                    result
                        .coverage
                        .unevaluated_rules
                        .extend(applicable[index..].iter().map(|r| r.id.clone()));
                    fail(&mut result, ErrorCode::InternalError);
                    break;
                }
                result.findings.push(Finding {
                    rule_id: rule.id.clone(),
                    category: rule.category.clone(),
                    method: if rule.r#match == Match::PolicyText {
                        Method::CustomPolicy
                    } else {
                        Method::Model
                    },
                    score: Some(score),
                    reason_code: "MODEL_SCORE".into(),
                    spans: vec![],
                });
                let action = if score >= threshold {
                    result.reason_codes.push("POLICY_MATCH".into());
                    rule.action
                } else if rule.review_threshold.is_some_and(|low| score >= low) {
                    Action::Review
                } else {
                    Action::Allow
                };
                if rank(action) > rank(result.action) {
                    result.action = action;
                }
                continue;
            }
            if rule.category.as_deref() != Some("pii.email") {
                result.status = Status::Indeterminate;
                result.coverage.unevaluated_rules.push(rule.id.clone());
                result.reason_codes.push("DETECTOR_UNAVAILABLE".into());
                continue;
            }
            let score = self.scorer.score(&normal, None, "pii.email");
            let matches: Vec<_> = crate::rules::EMAIL.find_iter(&normal).collect();
            match score {
                Ok(score) if score == if matches.is_empty() { 0.0 } else { 1.0 } => (),
                _ => {
                    result
                        .coverage
                        .unevaluated_rules
                        .extend(applicable[index..].iter().map(|r| r.id.clone()));
                    fail(&mut result, ErrorCode::InternalError);
                    break;
                }
            }
            if !matches.is_empty() {
                if request.options.include_spans
                    && result.findings.iter().map(|f| f.spans.len()).sum::<usize>() + matches.len()
                        > 512
                {
                    result
                        .coverage
                        .unevaluated_rules
                        .extend(applicable[index..].iter().map(|r| r.id.clone()));
                    fail(&mut result, ErrorCode::ResourceExhausted);
                    break;
                }
                let spans = if request.options.include_spans {
                    matches
                        .iter()
                        .map(|m| Span {
                            message_id: request.message.id.clone(),
                            start: mapping[m.start()].0,
                            end: mapping[m.end() - 1].1,
                        })
                        .collect()
                } else {
                    vec![]
                };
                result.findings.push(Finding {
                    rule_id: rule.id.clone(),
                    category: rule.category.clone(),
                    method: Method::Deterministic,
                    score: None,
                    reason_code: "EMAIL_PATTERN".into(),
                    spans,
                });
                if rank(rule.action) > rank(result.action) {
                    result.action = rule.action;
                }
                result.reason_codes.push("POLICY_MATCH".into());
            }
        }
        if result.status != Status::Assessed {
            result.action = Action::Review;
        }
        result.reason_codes.sort();
        result.reason_codes.dedup();
        result.duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        if started.elapsed().as_millis() >= u128::from(request.options.deadline_ms) {
            fail(&mut result, ErrorCode::DeadlineExceeded);
        }
        result
    }
}
fn rank(action: Action) -> u8 {
    match action {
        Action::Allow => 0,
        Action::Warn => 1,
        Action::Review => 2,
        Action::Block => 3,
    }
}
pub fn identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}
pub fn validate_policy(policy: &Policy) -> Result<(), ErrorCode> {
    if policy.schema_version != API_VERSION {
        return Err(ErrorCode::UnsupportedVersion);
    }
    if !identifier(&policy.id)
        || !identifier(&policy.version)
        || policy.version == "latest"
        || policy.rules.is_empty()
        || policy.rules.len() > 64
        || policy.default_action != Action::Allow
        || policy.on_error != Action::Review
        || policy.on_indeterminate != Action::Review
    {
        return Err(ErrorCode::InvalidPolicy);
    }
    let mut ids = HashSet::new();
    let mut unsupported = policy.profile != Profile::Personal;
    for rule in &policy.rules {
        if !identifier(&rule.id)
            || !ids.insert(&rule.id)
            || rule.directions.is_empty()
            || rule.directions.len() > 2
            || (rule.directions.len() == 2 && rule.directions[0] == rule.directions[1])
            || rule.action == Action::Allow
            || rule.category.as_ref().is_some_and(|category| !identifier(category))
        {
            return Err(ErrorCode::InvalidPolicy);
        }
        match rule.context_requirement {
            ContextRequirement::TargetOnly if rule.min_context_messages.is_some() => {
                return Err(ErrorCode::InvalidPolicy);
            }
            ContextRequirement::SuppliedWindow
                if !matches!(rule.min_context_messages, Some(1..=32)) =>
            {
                return Err(ErrorCode::InvalidPolicy);
            }
            _ => (),
        }
        match rule.r#match {
            Match::PolicyText => {
                if rule.category.is_some()
                    || rule
                        .policy_text
                        .as_ref()
                        .is_none_or(|p| p.is_empty() || p.len() > 512)
                    || rule.review_threshold.is_some()
                    || rule.action_threshold.is_some()
                    || rule.action == Action::Block
                    || (policy.profile == Profile::Platform && rule.action != Action::Warn)
                {
                    return Err(ErrorCode::InvalidPolicy);
                }
            }
            Match::Score => {
                let valid = match (rule.review_threshold, rule.action_threshold) {
                    (Some(low), Some(high)) => {
                        low.is_finite()
                            && high.is_finite()
                            && (0.0..=1.0).contains(&low)
                            && (0.0..=1.0).contains(&high)
                            && low < high
                    }
                    _ => false,
                };
                if !valid
                    || rule.category.is_none()
                    || rule.policy_text.is_some()
                    || rule.action == Action::Block
                {
                    return Err(ErrorCode::InvalidPolicy);
                }
            }
            Match::Detected => {
                if rule.policy_text.is_some()
                    || rule.category.is_none()
                    || rule.review_threshold.is_some()
                    || rule.action_threshold.is_some()
                {
                    return Err(ErrorCode::InvalidPolicy);
                }
                if rule.action == Action::Block {
                    unsupported = true;
                }
            }
        }
    }
    // This preview grants only personal warning authority.
    if unsupported {
        return Err(ErrorCode::UnsupportedPolicy);
    }
    Ok(())
}
pub fn validate_request(request: &Request) -> Result<(), ErrorCode> {
    if request.api_version != API_VERSION {
        return Err(ErrorCode::UnsupportedVersion);
    }
    let message = &request.message;
    if !identifier(&request.request_id)
        || !identifier(&message.id)
        || !identifier(&message.revision)
        || !identifier(&message.speaker)
        || message.text.is_empty()
        || message.text.len() > 16_384
        || request.context.len() > 32
        || !(1..=5000).contains(&request.options.deadline_ms)
        || request.language.len() > 32
        || request.language.is_empty()
    {
        return Err(ErrorCode::InvalidRequest);
    }
    let mut ids = HashSet::from([message.id.as_str()]);
    let mut total = message.text.len();
    for turn in &request.context {
        total += turn.text.len();
        if !identifier(&turn.id)
            || !identifier(&turn.speaker)
            || !ids.insert(&turn.id)
            || turn.text.is_empty()
            || turn.text.len() > 16_384
        {
            return Err(ErrorCode::InvalidRequest);
        }
    }
    if total > 65_536 {
        return Err(ErrorCode::InvalidRequest);
    }
    Ok(())
}
/// Compatibility normalisation for the ASCII email detector. Each produced byte
/// points back to the complete source code point, including width expansions.
fn normalize(text: &str) -> (String, Vec<(usize, usize)>) {
    let mut normalized = String::new();
    let mut mapping = Vec::new();
    for (start, character) in text.char_indices() {
        let end = start + character.len_utf8();
        let expanded: String = character.to_string().nfkc().collect();
        mapping.extend(std::iter::repeat_n((start, end), expanded.len()));
        normalized.push_str(&expanded);
    }
    (normalized, mapping)
}
pub fn base_result(request: &Request) -> Assessment {
    Assessment {
        api_version: API_VERSION.into(),
        request_id: request.request_id.clone(),
        message_id: request.message.id.clone(),
        message_revision: request.message.revision.clone(),
        status: Status::Error,
        action: Action::Review,
        versions: Versions {
            runtime: env!("CARGO_PKG_VERSION").into(),
            model: "none".into(),
            detectors: "0.1.0".into(),
            registry: "0.1.0".into(),
            policy_id: String::new(),
            policy_version: String::new(),
        },
        coverage: Coverage {
            language: request.language.clone(),
            target_complete: false,
            context_complete: false,
            dropped_context_ids: vec![],
            unevaluated_rules: vec![],
        },
        findings: vec![],
        reason_codes: vec![],
        duration_ms: 0,
        error_code: None,
    }
}
pub fn fail(result: &mut Assessment, code: ErrorCode) {
    result.status = Status::Error;
    result.action = Action::Review;
    result.error_code = Some(code);
    result.reason_codes = vec![
        serde_json::to_value(code)
            .expect("serializable error")
            .as_str()
            .expect("string error")
            .into(),
    ];
}

/// Decode an operation while preserving a valid correlation ID for typed errors.
/// Malformed policy values are INVALID_POLICY, distinct from malformed calls.
#[derive(Debug)]
pub struct DecodeError {
    pub call_id: String,
    pub error_code: ErrorCode,
}
pub fn decode_call(frame: &[u8]) -> Result<Call, DecodeError> {
    let error = |call_id, error_code| DecodeError {
        call_id,
        error_code,
    };
    if frame.is_empty() || frame.len() > MAX_FRAME {
        return Err(error(String::new(), ErrorCode::InvalidRequest));
    }
    match serde_json::from_slice::<Call>(frame) {
        Ok(call) => Ok(call),
        Err(_) => {
            let value: serde_json::Value = serde_json::from_slice(frame)
                .map_err(|_| error(String::new(), ErrorCode::InvalidRequest))?;
            let call_id = value
                .get("call_id")
                .and_then(|v| v.as_str())
                .filter(|id| identifier(id))
                .unwrap_or("")
                .to_owned();
            let mut code = ErrorCode::InvalidRequest;
            if let Some(root) = value.as_object()
                && root
                    .keys()
                    .all(|k| ["call_id", "api_version", "operation"].contains(&k.as_str()))
            {
                let operation = &value["operation"];
                let policy = match operation["op"].as_str() {
                    Some("validate_policy") => operation.get("policy"),
                    Some("assess") => operation
                        .get("request")
                        .and_then(|r| r.get("policy"))
                        .filter(|p| !p.is_null()),
                    _ => None,
                };
                if policy.is_some_and(|p| serde_json::from_value::<Policy>(p.clone()).is_err()) {
                    code = ErrorCode::InvalidPolicy;
                }
                if value["api_version"]
                    .as_str()
                    .is_some_and(|v| v != API_VERSION)
                {
                    code = ErrorCode::UnsupportedVersion;
                }
            }
            Err(error(call_id, code))
        }
    }
}
