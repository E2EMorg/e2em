//! E2EM runtime and Rust SDK: bounded local assessment, typed policies and results.
//! See [`runtime`] for the contract, embedded engine and asynchronous scheduler.
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};
use thiserror::Error;
mod rules;
pub mod runtime;
pub use rules::RulesScorer;

#[derive(Clone)]
pub struct RequestControl {
    pub deadline: Instant,
    pub cancelled: Arc<AtomicBool>,
}
impl RequestControl {
    pub fn stopped(&self) -> bool {
        self.cancelled.load(Ordering::Acquire) || Instant::now() >= self.deadline
    }
}

/// Interface for a trusted, bounded policy scorer.
pub trait PolicyScorer {
    fn begin_request(
        &self,
        model: Option<&str>,
        _control: RequestControl,
        _needs_model: bool,
    ) -> Result<(), BackendError> {
        if model.is_some() {
            return Err(BackendError::new("model selection unavailable"));
        }
        Ok(())
    }
    fn tokenizer_version(&self) -> String {
        "none".into()
    }
    fn max_tokens(&self) -> Option<usize> {
        None
    }
    fn coverage(&self) -> (bool, bool) {
        (true, true)
    }
    fn capabilities_model_version(&self) -> String {
        self.model_version()
    }
    /// Whether this backend can score arbitrary named policy categories.
    /// This describes execution support, not a policy's evaluation rating.
    fn supports_model_categories(&self) -> bool {
        false
    }

    /// Whether this backend can score arbitrary custom policy text.
    fn supports_custom_policies(&self) -> bool {
        false
    }

    /// Model identity for reports. Rules-only backends have no model.
    fn model_version(&self) -> String {
        "none".into()
    }

    fn score(
        &self,
        message: &str,
        context: Option<&str>,
        policy: &str,
    ) -> Result<f64, BackendError>;

    /// Scores one message against several policies, one probability per
    /// policy in request order. The default scores them one at a time; a
    /// backend that can answer them all in one pass (the preset-heads
    /// student) overrides it.
    fn score_many(
        &self,
        message: &str,
        context: Option<&str>,
        policies: &[String],
    ) -> Result<Vec<f64>, BackendError> {
        policies
            .iter()
            .map(|policy| self.score(message, context, policy))
            .collect()
    }

    /// A calibrated decision threshold for `policy`, if the backend ships one.
    fn action_threshold(&self, _policy: &str) -> Option<f64> {
        None
    }
    fn review_threshold(&self, _policy: &str) -> Option<f64> {
        None
    }
}

#[derive(Debug, Error)]
#[error("backend error: {message}")]
pub struct BackendError {
    message: String,
}

impl BackendError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}
