//! Transport-neutral 0.1 values. Unknown fields are rejected deliberately.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const API_VERSION: &str = "0.1";
pub const MAX_FRAME: usize = 131_072;
macro_rules! values {
    ($name:ident { $($variant:ident),+ }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($variant),+ }
    };
}
values!(Direction { Outgoing, Incoming });
values!(Profile { Personal, Platform });
values!(Override {
    UserConfirm,
    Forbidden
});
values!(ContextRequirement {
    TargetOnly,
    SuppliedWindow
});
values!(Match {
    Detected,
    Score,
    PolicyText
});
values!(Status {
    Assessed,
    Indeterminate,
    Error,
    Cancelled
});
values!(Action {
    Allow,
    Warn,
    Review,
    Block
});
values!(Method {
    Deterministic,
    Model,
    CustomPolicy
});
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    InvalidRequest,
    UnsupportedVersion,
    PolicyNotFound,
    InvalidPolicy,
    UnsupportedPolicy,
    ModelUnavailable,
    DeadlineExceeded,
    ResourceExhausted,
    InternalError,
}
impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ErrorCode {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    pub directions: Vec<Direction>,
    pub r#match: Match,
    pub context_requirement: ContextRequirement,
    pub action: Action,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_context_messages: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_threshold: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_threshold: Option<f64>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub schema_version: String,
    pub id: String,
    pub version: String,
    pub profile: Profile,
    pub rules: Vec<Rule>,
    pub default_action: Action,
    pub on_indeterminate: Action,
    pub on_error: Action,
    pub r#override: Override,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PolicyRef {
    pub provider: String,
    pub id: String,
    pub version: String,
    pub token: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Message {
    pub id: String,
    pub revision: String,
    pub speaker: String,
    pub text: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Turn {
    pub id: String,
    pub speaker: String,
    pub text: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Options {
    #[serde(default = "default_deadline")]
    pub deadline_ms: u64,
    #[serde(default)]
    pub include_spans: bool,
}
fn default_deadline() -> u64 {
    1000
}
impl Default for Options {
    fn default() -> Self {
        Self {
            deadline_ms: 1000,
            include_spans: false,
        }
    }
}
fn auto() -> String {
    "auto".into()
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub api_version: String,
    pub request_id: String,
    pub direction: Direction,
    pub message: Message,
    #[serde(default)]
    pub context: Vec<Turn>,
    #[serde(default = "auto")]
    pub language: String,
    #[serde(default)]
    pub options: Options,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_ref: Option<PolicyRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<Policy>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Span {
    pub message_id: String,
    pub start: usize,
    pub end: usize,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Finding {
    pub rule_id: String,
    pub category: Option<String>,
    pub method: Method,
    pub score: Option<f64>,
    pub reason_code: String,
    pub spans: Vec<Span>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Coverage {
    pub language: String,
    pub target_complete: bool,
    pub context_complete: bool,
    pub dropped_context_ids: Vec<String>,
    pub unevaluated_rules: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Versions {
    pub runtime: String,
    pub model: String,
    pub detectors: String,
    pub registry: String,
    pub policy_id: String,
    pub policy_version: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Assessment {
    pub api_version: String,
    pub request_id: String,
    pub message_id: String,
    pub message_revision: String,
    pub status: Status,
    pub action: Action,
    pub versions: Versions,
    pub coverage: Coverage,
    pub findings: Vec<Finding>,
    pub reason_codes: Vec<String>,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<ErrorCode>,
}
impl Assessment {
    /// Host applications must also invalidate on changes to context or policy.
    pub fn applies_to(&self, request: &Request) -> bool {
        self.request_id == request.request_id
            && self.message_id == request.message.id
            && self.message_revision == request.message.revision
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub message_bytes: usize,
    pub request_text_bytes: usize,
    pub context_turns: usize,
    pub policy_rules: usize,
    pub frame_bytes: usize,
    pub result_spans: usize,
    pub deadline_ms: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub api_version: String,
    pub provider: String,
    pub runtime: String,
    pub model: String,
    pub tokenizer: String,
    pub registry: String,
    pub languages: Vec<String>,
    pub detectors: Vec<String>,
    pub presets: Vec<String>,
    pub custom_policies: String,
    pub profiles: Vec<Profile>,
    pub actions: Vec<Action>,
    pub limits: Limits,
    pub backend_ready: bool,
    pub runtime_state: super::scheduler::State,
    pub max_tokens: Option<usize>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Capabilities,
    ValidatePolicy { policy: Policy },
    Assess { request: Box<Request> },
    Cancel { request_id: String },
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Call {
    pub call_id: String,
    pub api_version: String,
    pub operation: Operation,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Reply {
    Capabilities { capabilities: Capabilities },
    Policy { policy_ref: PolicyRef },
    Assessment { assessment: Assessment },
    Cancelled { accepted: bool },
    Error { error_code: ErrorCode },
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub call_id: String,
    pub reply: Reply,
}
