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
    /// A confirmation applies only to this snapshot and never grants future sends.
    pub fn can_continue(
        &self,
        assessment: &Assessment,
        current: &Request,
        confirmed: bool,
    ) -> bool {
        if &self.snapshot != current
            || !assessment.applies_to(current)
            || assessment.status != Status::Assessed
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
            Action::Allow => true,
            Action::Warn => {
                confirmed
                    && current
                        .policy
                        .as_ref()
                        .is_some_and(|p| p.r#override == Override::UserConfirm)
            }
            Action::Review | Action::Block => false,
        }
    }
}
