//! Verified provider selection; callers supply local adapter candidates only.
use super::*;
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProviderKind {
    Native,
    Project,
    Embedded,
}
#[derive(Debug, Clone)]
pub struct Candidate {
    pub kind: ProviderKind,
    pub authenticated: bool,
    pub authorised: bool,
    pub capabilities: Capabilities,
}
pub fn select<'a>(
    candidates: &'a [Candidate],
    pinned: Option<&str>,
    required_detectors: &[String],
) -> Result<&'a Candidate, ErrorCode> {
    candidates
        .iter()
        .filter(|c| pinned.is_none_or(|id| c.capabilities.provider == id))
        .filter(|c| {
            c.authenticated
                && c.authorised
                && c.capabilities.api_version == API_VERSION
                && c.capabilities.backend_ready
        })
        .filter(|c| {
            required_detectors
                .iter()
                .all(|d| c.capabilities.detectors.contains(d))
        })
        .min_by_key(|c| c.kind)
        .ok_or(ErrorCode::ModelUnavailable)
}
