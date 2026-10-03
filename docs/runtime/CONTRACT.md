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

Only `pii.email`, the personal profile and warn/review actions are enabled.
No contextual preset, custom rule, platform enforcement, model package or token
classifier is advertised. `model=none`, `tokenizer=none`, `max_tokens=null` and
empty presets accurately describe this slice. Unsupported rules fail validation
before any scorer call. Contextual and custom block requests are invalid even
when another unsupported rule precedes them. A deterministic block is unsupported
in this warning preview. No qualification is implied for Gandalf.

English (`en`) and language-neutral deterministic coverage (`und`) are exposed.
`auto` yields `und`; it does not claim language identification. An unsupported
explicit hint yields indeterminate/review. The full target is scanned, with no
prefix truncation or token limit. Width/compatibility normalisation maps each
output byte to its whole original code point. This mapping is appropriate to the
ASCII email detector; it is not a general tokenizer or linguistic normalizer.
Findings use null scores and half-open original UTF-8 byte spans, never matched
text. UTF-16 conversion helpers reject offsets inside a code point, including
emoji and BOM cases. This detector identifies email patterns, not verified
addresses or all possible email obfuscations.

The byte limits are 16,384 per turn and 65,536 total text; at most 32 unique prior
turns, 64 rules, 64 policy versions per principal, 512 result spans, and 131,072
bytes per IPC frame. Identifiers are nonempty, at most 128 bytes, without control
characters. Target text may include whitespace; it is never trimmed. Context
cannot repeat the target ID. IDs/revisions are opaque strings and `latest` is
invalid for a policy version. Options default individually to 1,000 ms and no
spans. Deadlines are 1–5,000 ms, include admission/validation, queueing, loading
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
keep confirmation specific to that snapshot. The Rust guard and SDK examples do
this. The application owns encryption/send/display and accessible warnings.
Installing this provider cannot compel an application to participate or obey.
`cargo run --locked --example runtime_chat` is an offline reference chat; it
requires explicit continuation for the email warning and holds failures.

## Compatibility and acceptance mapping

0.1 is a preview contract. Unknown fields are errors; additive optional fields
require an explicit negotiated schema revision before providers emit them.
Changing enum meanings, required fields, reference scope or ABI ownership is a
breaking revision. Clients reject incompatible API/ABI versions and malformed
responses. Separate API 0.1 from C ABI integer 1 and runtime package 0.1.0.

| Specification scenario | Evidence |
| --- | --- |
| Ordinary text, email [12,29), emoji, whitespace, full-width text, injection | `tests/conformance/assessments.json`, Rust/C/Python/Node parity |
| Missing context, exact bytes, stale revisions/context/policy | `tests/runtime.rs`, live SDK snapshot tests |
| Unsupported preset/custom; custom block; policy immutability/authority | `tests/conformance/policy-failures.json`, principal/reference tests |
| Deadline, memory pressure, load failure, overload, cancellation/restart | gated scheduler tests, live service tests |
| Malformed response/frame, wrong UID/secret/provider, revoked grant | SDK/schema and service black-box tests |
| Network/retention | Unix-only adapter; live tests inspect output and runtime files; no telemetry/download code |
| Tampered/downgraded models | No model loader/package is enabled. Production verification is issue #34 and remains a gate. |

This slice does not satisfy later signed-package, contextual-model or physical-
phone acceptance gates. It rejects unsupported capabilities rather than claiming
them through the contract.
