# Runtime roadmap

This repository tracks implementation and delivery of the developing [E2EM standard](https://e2em.org). Capability claims follow the running provider and conformance evidence.

The first focus is chat and chat messages: integrate assessment into the composer, warn before sending when a policy requires it, and keep decisions tied to the current draft. Integration pilots prioritize chat apps and their message send flows. All 40 built-in presets are selected by default; optional context, preset subsets, and inline custom text are supported. Model evaluation ratings describe quality and never act as an eligibility gate. Gandalf is the default model, with an exact `pii.email` detector; unavailable coverage remains visible.

| Stage | Scope | Status |
| --- | --- | --- |
| 0.1 developer preview | Local chat message assessment, message-only SDKs with 40 built-in presets, optional context and inline custom text, personal `pii.email` warnings, supplied embedded model scorer adapters; Rust/C embedding; Python/Node IPC; bounded scheduling; native packaging | Implemented, release checks qualify each artifact |
| Distribution | Signed Windows/macOS installers, notarization, automated user setup, registry publication | Planned |
| Integration pilots | Chat app adapters and message send flows, usability/accessibility checks, deployment security review | Planned |
| Broader capabilities | Contextual model distribution, published per-category evaluation ratings, and backend provisioning with verified assets | Gandalf distribution and provisioning shipped; per-category evaluation ratings remain planned |
| Platform providers | Reviewed sandbox/broker access and native OS integration | Planned |
| Browser bridge | Explicit extension transport and origin enrolment | Planned; interface only |

There are no committed dates. Execution capabilities remain truthful in `capabilities()`. Named categories are accepted without a quality allowlist; unavailable checks are reported as unevaluated. Custom text is accepted for reporting; unsupported authority still fails validation. Raise focused [issues](https://github.com/E2EMorg/e2em/issues) for implementation proposals.
