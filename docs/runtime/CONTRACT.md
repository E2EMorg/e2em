# E2EM contract 0.1 developer preview

The transport-neutral IDL is `src/runtime/contract.rs`. Generated structural JSON
schemas are in `schemas/`. Regenerate with `cargo run --locked --example
export_runtime_schema` and `python3 scripts/generate_runtime_bindings.py`.
Rust is the source of truth; Python TypedDicts and TypeScript unions are generated.
Semantic limits and policy authority are validated by the runtime, beyond the
structural schemas. Unknown fields, duplicate JSON fields and unknown enum values
are rejected by the provider. JSON is UTF-8; original message bytes are preserved.

## Operations and lifecycle

`capabilities()` reports the actual instance, readiness/state, rules, profiles,
languages and limits. `validatePolicy(policy)` returns an immutable, principal-
and provider-instance-scoped reference. `assess(request)` accepts exactly one
inline policy or reference and completes asynchronously. `cancel(request_id)` is
best effort, scoped to the owner; its boolean distinguishes accepted cancellation
from absent/completed work. Closing a client abandons its outstanding requests.
A restart or provider migration requires policy revalidation; SDKs never retry
or switch providers silently. Client discovery accepts provisioned local adapter
candidates, prefers compatible authenticated native candidates, then project and
embedded candidates, and supports pinning. No actual OS E2EM provider is claimed.

Rust exposes `Engine` for blocking embedding, `Scheduler` for bounded asynchronous
work (`Pending` is a Future, with blocking/polling helpers), and `RevisionGuard`
for the reference host send boundary. C ABI 1 exposes the same operations as
bounded Call JSON and Reply JSON, with owned result handles; see
[embedding instructions](../../crates/e2em-ffi/README.md).
Desktop IPC wraps operations in Call and Response envelopes, both carrying a
call ID so concurrent operations can complete out of order.

## Preview capability profile

The first integration focus is chat drafts and their send boundary. All named
categories are accepted for reporting, independently of model ratings. The service uses Gandalf by default and an exact email detector; the host displays advisory warnings without requiring confirmation.

Named categories support `detected` and `score` rules without a category allowlist
or model evaluation rating gate. Category identifiers are nonempty, at most 128
bytes, and have no control characters. Only the personal profile and warn/review
authority are enabled. Custom policy text is accepted for reporting; platform
enforcement remains unsupported.
The service reports its selected default model and tokenizer identity with `max_tokens=512`. The explicit diagnostic rules-only mode reports `model=none`, `tokenizer=none` and `max_tokens=null`. `presets` lists all 40 built-in definitions, regardless of evaluation
ratings or execution availability. `custom_policies=report-only` means text is
accepted but requires a model; a suitable loaded backend reports `supported`. Unavailable deterministic
checks report `DETECTOR_UNAVAILABLE`; unavailable model checks report
`MODEL_UNAVAILABLE`. Both produce indeterminate/review and enumerate unevaluated
rule IDs. A supplied embedded model scorer can opt into all named categories,
custom policy text, and report its model version. Known names resolve to canonical
preset wording. Compatible model/custom rules use the batch scoring operation;
optional context is shared within each batch. Every finite probability from 0 to 1 is returned as a
model finding, including scores below decision thresholds. Thresholds choose
message actions, not category eligibility. Contextual and custom block requests
are invalid even when an unavailable category precedes them. A deterministic
block is unsupported in this warning preview. Model evaluation ratings are
advisory; no model quality rating is claimed here.

English (`en`) and language-neutral deterministic coverage (`und`) are exposed.
`auto` yields `und`; it does not claim language identification. An unsupported
explicit hint yields indeterminate/review. The email detector scans the full target. Model evidence is bounded to 512 tokens; any truncation produces incomplete coverage and `MODEL_TOKEN_LIMIT`. Width/compatibility normalisation maps each
output byte to its whole original code point. This mapping is appropriate to the
ASCII email detector; it is not a general tokenizer or linguistic normalizer.
Deterministic findings use null scores and half-open original UTF-8 byte spans,
never matched text. Model findings include numeric scores and no spans. UTF-16
conversion helpers reject offsets inside a code point, including
emoji and BOM cases. This detector identifies email patterns, not verified
addresses or all possible email obfuscations.

The byte limits are 16,384 per turn and 65,536 total text; at most 32 unique prior
turns, 64 rules, 64 policy versions per principal, 512 result spans, and 131,072
bytes per IPC frame. Identifiers are nonempty, at most 128 bytes, without control
characters. Target text may include whitespace; it is never trimmed. Context
cannot repeat the target ID. IDs/revisions are opaque strings and `latest` is
invalid for a policy version. Options default individually to 15,000 ms, no spans and the configured default model. Deadlines are 1–30,000 ms, include admission/validation, queueing, loading
and scoring, and are checked before scoring and before publication.

`target_only` requires no history; `supplied_window` requires its declared 1–32
prior turns, and missing context yields indeterminate/review. This only verifies
the supplied window. Neither the runtime nor speaker aliases can authenticate
conversation completeness, age or identity.

## Status and integration

Assessed means every applicable rule completed its required coverage. Actions
aggregate block > review > warn > allow; only advertised actions may be requested.
An assessed review is a completed policy action. Indeterminate, error and
cancelled always produce review, retain available versions/coverage and cannot
authorize an automatic send. Partial findings do not override incomplete required
coverage. Backend inconsistency, load failure, overload, expiry and malformed
responses fail safely. A validation/admission failure can be an operation-level
Error reply before an assessment handle exists.

Results bind to request, message and revision. The host must additionally compare
its immutable text, context and policy snapshot, invalidate after any edit, and
recheck at the send boundary. Complete, current `allow` and `warn` results may
continue automatically. Warnings never require confirmation. The legacy policy
`override` field and Rust guard confirmation argument remain accepted for compatibility
but do not gate advisory warnings. The Rust guard and SDK examples implement this.
The application owns encryption/send/display and accessible warnings.
Installing this provider cannot compel an application to participate or obey.
`cargo run --locked --example runtime_chat` is an offline reference chat; it
uses every applicable preset by default, accepts optional subset/custom/context
flags or a policy JSON path, and prints model scores and unevaluated rules,
continues automatically after advisory warnings, and holds incomplete assessments.

## Compatibility and acceptance mapping

0.1 is a preview contract. Unknown fields are errors; additive optional fields
require an explicit negotiated schema revision before providers emit them.
Changing enum meanings, required fields, reference scope or ABI ownership is a
breaking revision. Clients reject incompatible API/ABI versions and malformed
responses. Separate API 0.1 from C ABI integer 1 and runtime package 0.1.3.

| Specification scenario | Evidence |
| --- | --- |
| Ordinary text, email [12,29), emoji, whitespace, full-width text, injection | `tests/conformance/assessments.json`, Rust/C/Python/Node parity |
| Missing context, exact bytes, stale revisions/context/policy | `tests/runtime.rs`, live SDK snapshot tests |
| Default presets; named/custom checks; optional context; unavailable checks; policy immutability/authority | `tests/conformance/policy-reports.json`, `tests/conformance/policy-failures.json`, model scorer and principal/reference tests |
| Deadline, memory pressure, load failure, overload, cancellation/restart | gated scheduler tests, live service tests |
| Malformed response/frame, wrong UID/secret/provider, revoked grant | SDK/schema and service black-box tests |
| Network/retention | Local assessment and no telemetry; optional idle daemon updates use canonical GitHub HTTPS without request data ([details](UPDATING.md)) |
| Tampered/downgraded models | Signed default manifests, hash-checked assets, monotonic model updates, candidate smoke checks and native qualification |

Model delivery and native CPU contextual scoring are shipped; installer signing and physical-phone qualification remain separate milestones. It reports unavailable category checks as unevaluated
and rejects unsupported authority and malformed policies.

`options.model` selects an owner-registered alias. `models()` lists aliases; `install_model(source,name,auto_update)` requires a model-management grant. Model downloads are outside assessment. `model_thresholds=true` on score rules applies matching package thresholds, falling back to the explicit rule values. Default message builders use this option. [Deployment and model controls](MODELS.md).
