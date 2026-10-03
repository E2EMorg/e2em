//! Rules-first embedded runtime, distinct from the research audit API.
pub mod contract;
pub mod discovery;
pub mod integration;
pub mod model;
#[cfg(feature = "runtime-service")]
pub mod models;
pub mod presets;
pub mod process;
pub mod scheduler;
#[cfg(feature = "runtime-service")]
pub mod service;
#[cfg(feature = "runtime-service")]
pub mod update;

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
            model: self.scorer.capabilities_model_version(),
            tokenizer: self.scorer.tokenizer_version(),
            registry: presets::registry_version().into(),
            languages: vec!["en".into(), "und".into()],
            detectors: vec!["pii.email".into()],
            presets: presets::all()
                .iter()
                .map(|preset| preset.id.clone())
                .collect(),
            custom_policies: if self.scorer.supports_custom_policies() {
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
                deadline_ms: 30000,
            },
            backend_ready: true,
            runtime_state: scheduler::State::Ready,
            max_tokens: self.scorer.max_tokens(),
        }
    }
    /// Assess a message against every built-in preset, with no service credentials.
    pub fn check(&self, message: &str) -> Assessment {
        self.check_with(message, presets::CheckOptions::default())
    }
    pub fn check_with(&self, message: &str, options: presets::CheckOptions) -> Assessment {
        self.assess("embedded", &presets::request(message, options))
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
        self.assess_controlled(
            principal,
            request,
            crate::RequestControl {
                deadline: Instant::now()
                    + std::time::Duration::from_millis(request.options.deadline_ms.min(30000)),
                cancelled: std::sync::Arc::default(),
            },
        )
    }
    pub fn assess_controlled(
        &self,
        principal: &str,
        request: &Request,
        control: crate::RequestControl,
    ) -> Assessment {
        let started = Instant::now();
        let mut result = base_result(request);
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
        let needs_model = policy
            .rules
            .iter()
            .any(|r| r.directions.contains(&request.direction) && r.r#match != Match::Detected);
        if self
            .scorer
            .begin_request(request.options.model.as_deref(), control, needs_model)
            .is_err()
        {
            fail(&mut result, ErrorCode::ModelUnavailable);
            return result;
        }
        result.versions.model = self.scorer.model_version();
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
        let mut model_scores = vec![None; applicable.len()];
        let mut scored_contexts = HashSet::new();
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
                let custom = rule.r#match == Match::PolicyText;
                if (custom && !self.scorer.supports_custom_policies())
                    || (!custom && !self.scorer.supports_model_categories())
                {
                    result.status = Status::Indeterminate;
                    result.coverage.unevaluated_rules.push(rule.id.clone());
                    result.reason_codes.push("MODEL_UNAVAILABLE".into());
                    continue;
                }
                let policy_text = if custom {
                    rule.policy_text.as_deref().expect("validated policy text")
                } else {
                    let category = rule.category.as_deref().expect("validated category");
                    presets::find(category).map_or(category, |preset| preset.wording.as_str())
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
                let contextual = rule.context_requirement == ContextRequirement::SuppliedWindow;
                if scored_contexts.insert(contextual) {
                    let group: Vec<_> = applicable
                        .iter()
                        .enumerate()
                        .filter(|(_, candidate)| {
                            candidate.r#match != Match::Detected
                                && (candidate.context_requirement
                                    == ContextRequirement::SuppliedWindow)
                                    == contextual
                                && (!contextual
                                    || request.context.len()
                                        >= candidate.min_context_messages.unwrap_or(1))
                                && if candidate.r#match == Match::PolicyText {
                                    self.scorer.supports_custom_policies()
                                } else {
                                    self.scorer.supports_model_categories()
                                }
                        })
                        .collect();
                    let policies: Vec<String> = group
                        .iter()
                        .map(|(_, candidate)| {
                            if candidate.r#match == Match::PolicyText {
                                candidate.policy_text.clone().expect("validated text")
                            } else {
                                let category =
                                    candidate.category.as_deref().expect("validated category");
                                presets::find(category)
                                    .map_or(category, |preset| preset.wording.as_str())
                                    .into()
                            }
                        })
                        .collect();
                    match self.scorer.score_many(
                        &request.message.text,
                        context.as_deref(),
                        &policies,
                    ) {
                        Ok(scores)
                            if scores.len() == group.len()
                                && scores.iter().all(|score| {
                                    score.is_finite() && (0.0..=1.0).contains(score)
                                }) =>
                        {
                            for ((position, _), score) in group.into_iter().zip(scores) {
                                model_scores[position] = Some(score);
                            }
                        }
                        _ => {
                            result
                                .coverage
                                .unevaluated_rules
                                .extend(applicable[index..].iter().map(|r| r.id.clone()));
                            fail(&mut result, ErrorCode::InternalError);
                            break;
                        }
                    }
                }
                let score = model_scores[index].expect("successful batch includes this rule");
                let threshold = if custom {
                    self.scorer.action_threshold(policy_text).unwrap_or(0.5)
                } else if rule.model_thresholds {
                    self.scorer
                        .action_threshold(policy_text)
                        .unwrap_or(rule.action_threshold.expect("validated threshold"))
                } else {
                    rule.action_threshold.expect("validated threshold")
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
                    method: if custom {
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
                } else if (if rule.model_thresholds {
                    self.scorer
                        .review_threshold(policy_text)
                        .or(rule.review_threshold)
                } else {
                    rule.review_threshold
                })
                .is_some_and(|low| score >= low)
                {
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
            let score = if self.scorer.supports_model_categories()
                || self.scorer.supports_custom_policies()
            {
                RulesScorer::new().score(&normal, None, "pii.email")
            } else {
                self.scorer.score(&normal, None, "pii.email")
            };
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
        let coverage = self.scorer.coverage();
        if matches!(result.status, Status::Assessed | Status::Indeterminate)
            && (!coverage.0 || !coverage.1)
        {
            result.coverage.target_complete &= coverage.0;
            result.coverage.context_complete &= coverage.1;
            result.status = Status::Indeterminate;
            result.reason_codes.push("MODEL_TOKEN_LIMIT".into());
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
            || rule
                .category
                .as_ref()
                .is_some_and(|category| !identifier(category))
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
                if rule.model_thresholds
                    || rule.category.is_some()
                    || rule
                        .policy_text
                        .as_ref()
                        .is_none_or(|p| p.trim().is_empty() || p.len() > 512)
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
                if rule.model_thresholds
                    || rule.policy_text.is_some()
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
        || !(1..=30000).contains(&request.options.deadline_ms)
        || request
            .options
            .model
            .as_ref()
            .is_some_and(|m| !model::name(m))
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
            registry: presets::registry_version().into(),
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
