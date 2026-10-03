//! Built-in policy definitions; model ratings never remove entries.
use super::*;
use serde::Deserialize;
use std::sync::LazyLock;

#[derive(Debug, Deserialize)]
pub struct Preset {
    pub id: String,
    pub tier: String,
    pub directions: Vec<Direction>,
    pub wording: String,
}
#[derive(Deserialize)]
struct Catalogue {
    registry: String,
    presets: Vec<Preset>,
}
static CATALOGUE: LazyLock<Catalogue> = LazyLock::new(|| {
    serde_json::from_str(include_str!("presets.json")).expect("valid built-in presets")
});
pub fn all() -> &'static [Preset] {
    &CATALOGUE.presets
}
pub fn registry_version() -> &'static str {
    &CATALOGUE.registry
}
pub fn find(category: &str) -> Option<&'static Preset> {
    all().iter().find(|preset| preset.id == category)
}

/// Message assessment options. Omitting categories evaluates all built-in presets.
#[derive(Debug, Default, Clone)]
pub struct CheckOptions {
    pub context: Vec<String>,
    pub policies: Option<Vec<String>>,
    pub custom_policies: Vec<String>,
    pub model: Option<String>,
    pub deadline_ms: Option<u64>,
}

/// Build the wire request without requiring a caller to write policy JSON.
pub fn request(message: &str, options: CheckOptions) -> Request {
    let selected = options
        .policies
        .unwrap_or_else(|| all().iter().map(|preset| preset.id.clone()).collect());
    let contextual = !options.context.is_empty();
    let mut rules: Vec<_> = selected
        .into_iter()
        .enumerate()
        .map(|(index, category)| {
            let preset = find(&category);
            // Email has an exact detector; other defaults use the model fallback.
            let detected = category == "pii.email";
            let contextual = contextual || category == "spam.repeat";
            Rule {
                model_thresholds: !detected,
                id: format!("preset-{}", index + 1),
                directions: preset.map_or_else(
                    || vec![Direction::Outgoing, Direction::Incoming],
                    |preset| preset.directions.clone(),
                ),
                r#match: if detected {
                    Match::Detected
                } else {
                    Match::Score
                },
                context_requirement: if contextual && !detected {
                    ContextRequirement::SuppliedWindow
                } else {
                    ContextRequirement::TargetOnly
                },
                min_context_messages: (contextual && !detected).then_some(1),
                action: Action::Warn,
                category: Some(category),
                policy_text: None,
                review_threshold: (!detected).then_some(0.4),
                action_threshold: (!detected).then_some(0.7),
            }
        })
        .collect();
    rules.extend(
        options
            .custom_policies
            .into_iter()
            .enumerate()
            .map(|(index, text)| Rule {
                model_thresholds: false,
                id: format!("custom-{}", index + 1),
                directions: vec![Direction::Outgoing, Direction::Incoming],
                r#match: Match::PolicyText,
                context_requirement: if contextual {
                    ContextRequirement::SuppliedWindow
                } else {
                    ContextRequirement::TargetOnly
                },
                min_context_messages: contextual.then_some(1),
                action: Action::Warn,
                category: None,
                policy_text: Some(text),
                review_threshold: None,
                action_threshold: None,
            }),
    );
    let policy = Policy {
        schema_version: API_VERSION.into(),
        id: "message-check".into(),
        version: "1".into(),
        profile: Profile::Personal,
        rules,
        default_action: Action::Allow,
        on_indeterminate: Action::Review,
        on_error: Action::Review,
        r#override: Override::UserConfirm,
    };
    Request {
        api_version: API_VERSION.into(),
        request_id: format!(
            "check-{}",
            super::INSTANCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ),
        direction: Direction::Outgoing,
        message: Message {
            id: "message".into(),
            revision: "1".into(),
            speaker: "self".into(),
            text: message.into(),
        },
        context: options
            .context
            .into_iter()
            .enumerate()
            .map(|(index, text)| Turn {
                id: format!("context-{}", index + 1),
                speaker: "peer".into(),
                text,
            })
            .collect(),
        language: "auto".into(),
        options: Options {
            model: options.model,
            deadline_ms: options.deadline_ms.unwrap_or(15000),
            ..Options::default()
        },
        policy: Some(policy),
        policy_ref: None,
    }
}
