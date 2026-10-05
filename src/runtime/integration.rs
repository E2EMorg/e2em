//! Reference host integration guard. It never performs a send or display.
use super::*;
#[derive(Clone)]
pub struct RevisionGuard {
    snapshot: Request,
}
impl RevisionGuard {
    pub fn new(snapshot: Request) -> Self {
        Self { snapshot }
    }
    /// Bind continuation to the complete request, including context and policy.
    /// Warnings are advisory and require no user confirmation.
    /// The legacy confirmation argument is ignored for source compatibility.
    pub fn can_continue(
        &self,
        assessment: &Assessment,
        current: &Request,
        _confirmed: bool,
    ) -> bool {
        if &self.snapshot != current
            || !assessment.applies_to(current)
            || assessment.status != Status::Assessed
            || !assessment.coverage.target_complete
            || !assessment.coverage.context_complete
            || !assessment.coverage.unevaluated_rules.is_empty()
        {
            return false;
        }
        let (id, version) = if let Some(policy) = &current.policy {
            (&policy.id, &policy.version)
        } else if let Some(reference) = &current.policy_ref {
            (&reference.id, &reference.version)
        } else {
            return false;
        };
        if &assessment.versions.policy_id != id || &assessment.versions.policy_version != version {
            return false;
        }
        match assessment.action {
            Action::Allow | Action::Warn => true,
            Action::Review | Action::Block => false,
        }
    }
}
