# Chat SDK integration guide

[Runtime setup](INSTALL.md) · [Contract](runtime/CONTRACT.md) · [Downloads](https://github.com/E2EMorg/e2em/releases)

Send a message and get a report. All built-in presets are selected automatically. Context is optional, and you only specify policies when you want a subset or custom rules. Model ratings never decide which presets you can try.

The service uses Gandalf by default and includes all 40 presets. It scores custom text and supports registered custom model selection through `model` options. [Normal/offline provisioning and model format](runtime/MODELS.md).

After [one-time runtime setup](INSTALL.md), use the SDK's `app` option to
identify your application. The SDK automatically registers a new app with the
installed runtime and loads its private local settings. No permission prompt,
browser, or **Connect app** approval is required. Existing credentials are
preserved. Explicit `config_path` / `configPath` settings are loaded as supplied.
For runtime setup launched from an app, `e2emd --setup --setup-app YOUR_APP_NAME`
also connects the app automatically. Windows integrations can use the console-free
`e2em-setup.exe --setup-app YOUR_APP_NAME` launcher beside the installed daemon.

## Python

Install the SDK from a checkout or a downloaded release package:

```sh
python3 -m pip install ./sdk/python
```

After [one-time runtime setup](INSTALL.md):

```python
from e2em import assess

report = assess("Hello there!")
print(report.scores)
print(report.unevaluated)
```

Add earlier conversation, choose a subset, or add custom text:

```python
assess("That sounds good", context=["Shall we meet tomorrow?"])
assess("Hello there!", policies=["identity.hate", "abuse.threat"])
assess("The launch is next week", custom_policies=["Keep launch dates private."])
```

Custom policies add to the defaults. For custom-only assessment, pass `policies=[]`. `context`, `policies`, and `custom_policies` each accept a single string or a list of strings. Context lists are ordered from oldest to newest.

For a chat app that sends many requests, keep one client open:

```python
from e2em import BlockingClient

with BlockingClient(app="my-app") as client:
    report = client.assess("Hello there!", context=["Earlier message"])
```

Async apps use `async with await Client.connect(app="my-app") as client`, then `await client.assess(message, context=...)`. Python 3.11+; no inference dependencies.

## Node and TypeScript

Install from a checkout or a downloaded release archive:

```sh
npm install ./sdk/node
```

```javascript
import {assess} from '@e2em/local';

const report = await assess('Hello there!');
console.log(report.scores, report.unevaluated);
```

```javascript
await assess('That sounds good', {context: ['Shall we meet tomorrow?']});
await assess('Hello there!', {policies: ['identity.hate', 'abuse.threat']});
await assess('The launch is next week', {customPolicies: ['Keep launch dates private.']});
```

Use `policies: []` for custom-only assessment. The three options accept a string or an array of strings. For a persistent connection:

```javascript
import {Client} from '@e2em/local';

const client = await Client.connect({app: 'my-app'});
try {
  const report = await client.assess('Hello there!', {context: ['Earlier message']});
  console.log(report.scores);
} finally {
  client.close();
}
```

Node.js 22+; generated TypeScript types are included. `signal` supports cancellation. The browser entry point provides an explicit bridge interface; no browser transport is shipped.

## Dart and Flutter desktop

Linux, macOS, and Windows are supported. Mobile and web are outside this preview.
Add a path dependency on the checkout's `sdk/dart` directory:

```yaml
dependencies:
  e2em_local:
    path: /path/to/e2em/sdk/dart
```

After one-time runtime setup:

```dart
import 'package:e2em_local/e2em.dart';

final report = await assess('Hello there!', app: 'daccord');
print(report.scores);
print(report.unevaluated);
```

Use `context`, `policies`, and `customPolicies` lists for earlier conversation,
preset subsets, and custom text. Use `policies: []` for custom-only assessment.
Keep one `Client.connect(app: 'daccord')` open for a chat session and close it in
`finally`. A `CancellationToken` supports cancellation; `report.appliesTo` checks
the current complete request snapshot before sending. Dart 3.5+; no inference
dependencies. The [Dart SDK guide](../sdk/dart/README.md) includes advanced
requests, discovery, model operations, and a send-flow example.

## Rust

Embedded use needs no running service, app enrolment, or credentials:

```rust
use e2em_runtime::runtime::{Engine, presets::CheckOptions};

fn main() {
    let engine = Engine::default();
    let report = engine.check("Hello there!");
    println!("{:?}", report.findings);

    let report = engine.check_with("That sounds good", CheckOptions {
        context: vec!["Shall we meet tomorrow?".into()],
        custom_policies: vec!["Keep launch dates private.".into()],
        ..Default::default()
    });
    println!("{:?}", report.coverage.unevaluated_rules);
}
```

Rust 1.95+. Use an `e2em-runtime` path or tagged Git dependency. `Scheduler::check` and `check_with` return asynchronous or blocking pending assessments. The [reference chat](../examples/runtime_chat.rs) supports repeated flags:

```sh
cargo run --locked --example runtime_chat
cargo run --locked --example runtime_chat -- --policy identity.hate --policy abuse.threat
cargo run --locked --example runtime_chat -- --custom "Keep launch dates private." --context "Earlier message"
```

## Presets, scores, and custom text

The [built-in catalogue](../src/runtime/presets.json) includes all 40 definitions, including launch and candidate categories. Every applicable default is selected; direction-specific presets apply to their declared direction. The simple API assesses outgoing messages. Use the advanced request API for incoming messages and explicit turn IDs or speaker aliases.

A supplied `PolicyScorer` opts into preset scoring with `supports_model_categories()` and custom text with `supports_custom_policies()`. Registered preset IDs are resolved to canonical wording; unregistered names are passed through. Optional context reaches model/custom scoring without rewriting the target text. Compatible rules use `score_many` together so a backend with preset heads can evaluate them in one pass. `model_version()` identifies the supplied model.

Every finite score from 0 to 1 is reported, including zero. These are message scores, separate from model quality ratings. Preset message thresholds default to review at 0.4 and the rule's warning at 0.7; advanced policy JSON can change them. Custom text uses the backend's action threshold, or 0.5 if none is supplied. No evaluation rating gate exists.

Unavailable model/custom checks report `MODEL_UNAVAILABLE`; unimplemented deterministic checks report `DETECTOR_UNAVAILABLE`. Both produce `indeterminate` / `review`, retain completed findings, and enumerate unevaluated rules. The simple SDK report maps those rule IDs to preset names in `report.unevaluated`. Custom-only entries use IDs such as `custom-1`.

## Local service setup

Python, Node, and Dart connect to the installed local service. `app` defaults to `my-app`, matching the [setup guide](INSTALL.md). `Client.connect`, `BlockingClient`, and standalone `assess` register new apps and load their local settings automatically. There is no permission prompt, account sign-in, remote service, or manual credential argument in the quick start.

The service authenticates each app to keep its policies and requests separate from other registered apps. Its private local token is created automatically by the installed runtime and read by the SDK. Embedded Rust/C assessment runs inside your app and does not need this token. Installing the runtime alone does not start it; complete runtime setup once before using the service.

Default settings for `my-app` are stored at `~/.config/e2em/apps/my-app.json` on Linux/macOS and `%LOCALAPPDATA%\E2EM\app-my-app.json` on Windows. Use `config_path` in Python or `configPath` in Node/Dart for an explicitly provisioned alternative. SDK loaders check private Unix file ownership and permissions. Keep these installer-managed settings out of source control.

## Advanced integration and policy files

Files are optional. For bulk rules, strict message thresholds, policy versioning, explicit turn IDs, directions, or revisions, use the existing `Client.assess(request)` API and `validate_policy` / `validatePolicy`. [`chat-policy.json`](../examples/chat-policy.json) and [`category-policy.json`](../examples/category-policy.json) remain examples. The chat demo accepts `--policy-file path.json`; the existing positional file argument remains compatible.

`Client.open` still accepts explicit provisioned connection details; `discover` accepts a candidate list and an optional required-detector list. Requiring a detector means the chosen provider must actually implement it. Do not silently retry or switch providers on errors.

Before sending, compare the report with the current message, context, and policy snapshot. Simple reports expose `report.request`; use Python `applies_to(current_request)` or Node/Dart `appliesTo(currentRequest)`. Invalidate after any edit. `integration::RevisionGuard` provides the Rust send guard. A complete, current `allow` or `warn` may proceed automatically; warnings are advisory and require no confirmation. Hold errors, cancellations, stale results, incomplete coverage, and `review` for editing or review. Recheck the snapshot at the send boundary.

Context is optional. Advanced rules can require a supplied window; missing required context produces an indeterminate report. Limits are 32 prior messages, 16,384 UTF-8 bytes per message, 65,536 total text bytes, 64 rules, and 512 bytes per custom policy text. See the [contract](runtime/CONTRACT.md) for deadlines and full semantics. The service grants personal warning/review authority; platform enforcement and block policies remain unsupported.

## C and C++

Download a native C SDK archive or build with `cargo build --locked -p e2em-ffi`. Include [`e2em.h`](../crates/e2em-ffi/include/e2em.h), link `e2em_ffi`, and use UTF-8 Call JSON. The C ABI is an advanced embedding interface and has no service credentials. The [ownership guide](../crates/e2em-ffi/README.md) and [examples](../crates/e2em-ffi/examples) cover polling and handle lifetimes.

Spans are half-open UTF-8 byte offsets into original text. Python `utf16_span` and Node/Dart `utf16Span` convert spans for UTF-16 UIs and reject code point splits. Runtime errors do not echo message text. SDK errors expose fixed codes; unavailable providers and malformed replies hold the draft for review. The application owns sending, encryption, display, and logging. See the [security boundaries](runtime/SECURITY.md).

SDK packages are attached to [GitHub Releases](https://github.com/E2EMorg/e2em/releases). PyPI, npm, pub.dev, and crates.io publication is not enabled.
