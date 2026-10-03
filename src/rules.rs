//! The runtime preview's deterministic email detector.
use crate::{BackendError, PolicyScorer};
use regex::Regex;
use std::sync::LazyLock;
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

pub(crate) static EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b[a-z0-9._%+\-]+@[a-z0-9\-]+(?:\.[a-z0-9\-]+)*\.[a-z]{2,}\b").expect("valid")
});

/// Rules-only scorer for the preview's declared `pii.email` capability.
#[derive(Debug, Clone, Copy, Default)]
pub struct RulesScorer;
impl RulesScorer {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}
impl PolicyScorer for RulesScorer {
    fn score(
        &self,
        message: &str,
        _context: Option<&str>,
        policy: &str,
    ) -> Result<f64, BackendError> {
        if policy != "pii.email" {
            return Err(BackendError::new("unsupported rule"));
        }
        // Preserve the original detector's normalization. Runtime span mapping
        // remains in the engine; this scorer returns no matched text.
        let evidence: String = message.nfkc().filter(|c| !matches!(c,
            '\u{200B}'..='\u{200F}' | '\u{2060}' | '\u{FEFF}' | '\u{00AD}' | '\u{180E}' | '\u{2028}'..='\u{202E}'
        ) && !is_combining_mark(*c)).collect();
        Ok(if EMAIL.is_match(&evidence) { 1.0 } else { 0.0 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scores_email_without_research_registry_or_models() {
        let scorer = RulesScorer::new();
        assert_eq!(
            scorer
                .score("mail alex@example.test", None, "pii.email")
                .unwrap(),
            1.0
        );
        assert_eq!(scorer.score("hello", None, "pii.email").unwrap(), 0.0);
        assert!(scorer.score("hello", None, "identity.hate").is_err());
    }
}
