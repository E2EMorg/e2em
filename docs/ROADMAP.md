# Runtime roadmap

This repository tracks implementation and delivery of the developing [E2EM standard](https://e2em.org). Capability claims follow the running provider and conformance evidence.

The first focus is chat and chat messages: integrate assessment into the composer, warn before sending when a policy requires it, and keep decisions tied to the current draft. Integration pilots prioritize chat apps and their message send flows. The initial `pii.email` rule checks for email addresses shared in chat; it is a starting capability for this flow.

| Stage | Scope | Status |
| --- | --- | --- |
| 0.1 developer preview | Local chat message assessment with personal `pii.email` warnings; Rust/C embedding; Python/Node IPC; bounded scheduling; native packaging | Implemented, release checks qualify each artifact |
| Distribution | Signed Windows/macOS installers, notarization, automated user setup, registry publication | Planned |
| Integration pilots | Chat app adapters and message send flows, usability/accessibility checks, deployment security review | Planned |
| Broader capabilities | Qualified contextual models and backend provisioning with verified assets | Planned; no model shipped |
| Platform providers | Reviewed sandbox/broker access and native OS integration | Planned |
| Browser bridge | Explicit extension transport and origin enrolment | Planned; interface only |

There are no committed dates. Unsupported capabilities remain absent from `capabilities()` and fail validation. Raise focused [issues](https://github.com/E2EMorg/e2em/issues) for implementation proposals.
