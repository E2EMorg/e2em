# Chat SDK integration guide

[Runtime installation](INSTALL.md) · [Contract](runtime/CONTRACT.md) · [Downloads](https://github.com/E2EMorg/e2em/releases)

Integrate E2EM at your chat app's send boundary: assess a draft, show any warning, and confirm that the draft is still current before sending. All named policy categories are accepted for reporting; model evaluation ratings do not gate access. The bundled backend evaluates email-address patterns (`pii.email`). Other categories are reported as unevaluated unless a suitable backend is supplied.

Install and start the runtime, then enrol `my-app` using the [platform setup guide](INSTALL.md). Python and Node SDK packages are downloadable release assets; registry publication is not enabled. The Rust and C SDKs support embedding, which needs no running service or enrolment.

## Policy and credentials

[`examples/chat-policy.json`](../examples/chat-policy.json) is a minimal example policy for outgoing chat messages, not a required policy. It warns when a draft includes an email address, chooses `review` for errors and indeterminate results, and requires explicit user confirmation for warnings. Copy it into your application or load it from this checkout. The earlier [`email-policy.json`](../examples/email-policy.json) example remains available.

Private credentials created by enrolment contain `socket_path`, `principal`, `secret`, and `provider`:

| Platform | Credential file for `my-app` |
| --- | --- |
| Linux / macOS | `~/.config/e2em/apps/my-app.json` |
| Windows | `%LOCALAPPDATA%\E2EM\app-my-app.json` |

Never commit this file. In Node, pass `socket_path` as `socketPath`. The `kind` field is discovery metadata, not a connection argument.

## Python

Python 3.11+; no runtime dependencies outside the standard library.

```sh
python3 -m pip install ./sdk/python
# Or install the wheel downloaded from a GitHub Release:
python3 -m pip install ./e2em_local-0.1.1-py3-none-any.whl
```

```python
import asyncio
import json
import os
from pathlib import Path
from e2em import Client, E2EMError

async def main():
    credential_path = (
        Path(os.environ["LOCALAPPDATA"]) / "E2EM/app-my-app.json"
        if os.name == "nt" else Path.home() / ".config/e2em/apps/my-app.json"
    )
    config = json.loads(credential_path.read_text())
    policy = json.loads(Path("examples/chat-policy.json").read_text())
    client = await Client.open(**{key: config[key] for key in
        ("socket_path", "principal", "secret", "provider")})
    async with client:
        reference = await client.validate_policy(policy)
        request = {
            "api_version": "0.1", "request_id": "draft-1", "direction": "outgoing",
            "message": {"id": "draft", "revision": "1", "speaker": "self",
                        "text": "You can reach me at alex@example.test"},
            "policy_ref": reference,
        }
        try:
            result = await client.assess(request)
        except E2EMError as error:
            print(f"Hold for review: {error.code}")
            return
        if not result.applies_to(request):
            print("Draft changed; assess again")
            return
        print(result.status, result.action)  # assessed warn

asyncio.run(main())
```

The example reports a decision and sends nothing. [`send_flow.py`](../sdk/python/examples/send_flow.py) demonstrates a chat draft and user-confirmed warning flow using a provisioned candidate array and the chat policy. For synchronous applications, `BlockingClient` owns a dedicated event loop and exposes the same operations; close it explicitly.

## Node and TypeScript

Node.js 22+; no inference or external runtime dependencies. The SDK ships generated TypeScript contract types.

```sh
npm install ./sdk/node
# Or install the release asset:
npm install ./e2em-local-0.1.1.tgz
```

```javascript
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {Client} from '@e2em/local';

const credentialPath = process.platform === 'win32'
  ? path.join(process.env.LOCALAPPDATA, 'E2EM', 'app-my-app.json')
  : path.join(os.homedir(), '.config/e2em/apps/my-app.json');
const config = JSON.parse(await fs.readFile(credentialPath, 'utf8'));
const policy = JSON.parse(await fs.readFile('examples/chat-policy.json', 'utf8'));
const client = await Client.open({
  socketPath: config.socket_path,
  principal: config.principal, secret: config.secret, provider: config.provider,
});
try {
  const policy_ref = await client.validatePolicy(policy);
  const request = {
    api_version: '0.1', request_id: 'draft-1', direction: 'outgoing',
    message: {id: 'draft', revision: '1', speaker: 'self',
              text: 'You can reach me at alex@example.test'},
    policy_ref,
  };
  const result = await client.assess(request);
  if (!result.appliesTo(request)) throw new Error('Draft changed; assess again');
  console.log(result.status, result.action); // assessed warn
} catch (error) {
  console.error('Hold for review:', error.code ?? 'INTERNAL_ERROR');
} finally {
  client.close();
}
```

The Promise API also offers `capabilities()`, `cancel(requestId)`, and `assess(request, signal)` for cancellation. `discover()` takes explicitly provisioned candidates; it never silently downloads or switches providers. [`send-flow.js`](../sdk/node/examples/send-flow.js) demonstrates the chat warning confirmation step. The browser entry point is an explicit bridge interface; a browser extension transport is not shipped.

## Rust

Rust 1.95+. Use the tagged Git repository or unpack the Rust SDK source archive and use a path dependency:

```toml
[dependencies]
e2em-runtime = { git = "https://github.com/E2EMorg/e2em.git", tag = "v0.1.1", default-features = false }
serde_json = "1"
```

```rust
use e2em_runtime::runtime::*;

fn main() {
    let policy: Policy = serde_json::from_str(include_str!("chat-policy.json"))
        .expect("valid application policy");
    let engine = Engine::default();
    let reference = engine.validate_policy("my-app", policy)
        .expect("supported personal policy");
    let request = Request {
        api_version: API_VERSION.into(), request_id: "draft-1".into(),
        direction: Direction::Outgoing,
        message: Message {
            id: "draft".into(), revision: "1".into(), speaker: "self".into(),
            text: "You can reach me at alex@example.test".into(),
        },
        context: vec![], language: "en".into(), options: Options::default(),
        policy_ref: Some(reference), policy: None,
    };
    let result = engine.assess("my-app", &request);
    println!("{:?} {:?}", result.status, result.action); // Assessed Warn
}
```

Save the policy next to your Rust source. Production hosts should handle policy errors and recheck the current revision at their action boundary. The complete runnable example is [`runtime_chat.rs`](../examples/runtime_chat.rs):

```sh
cargo run --locked --example runtime_chat
```

For async integration use `runtime::scheduler::Scheduler`. `integration::RevisionGuard` protects the final host action from stale assessments. The default feature enables the daemon; turn it off for embedded-only use.

## C and C++

Download the native C SDK archive for your platform or build it from source:

```sh
cargo build --locked -p e2em-ffi
```

Include [`e2em.h`](../crates/e2em-ffi/include/e2em.h), link the shared/static `e2em_ffi` library, and send UTF-8 Call JSON. No Rust allocation crosses the ABI. C ABI 1 supports API 0.1. Poll and read result handles, free every handle, and close clients. See the [ownership and lifetime guide](../crates/e2em-ffi/README.md) and [C/C++ examples](../crates/e2em-ffi/examples).

Linux native C libraries are built for the release's Ubuntu 24.04 runner and can require its glibc baseline; the Linux daemon installers use static musl instead. Use a source build for older Linux hosts.

## Integration lifecycle

1. Open an authenticated client and check actual capabilities.
2. Validate your immutable, explicitly versioned policy. Store its reference only for this provider instance.
3. Snapshot the current chat draft, context, policy, IDs and revision, then assess.
4. If the draft changes, cancel its work where possible and submit the new revision.
5. Check the assessment against the **current** request immediately before acting.
6. Allow only `allow`, or an explicitly user-confirmed `warn` permitted by policy; hold `review`, errors and incomplete/stale results.
7. Reconnect and revalidate after provider restart or migration; close the client on disposal.

Spans are half-open UTF-8 byte offsets into original message text. Python `utf16_span` and Node `utf16Span` convert them for UTF-16 UIs and reject offsets inside code points. They do not rewrite text.

## Named category trials

Use [`examples/category-policy.json`](../examples/category-policy.json) to try multiple named categories. Replace its category names with any category you want to report; there is no approved-category list or minimum model evaluation rating. Category identifiers must be nonempty, at most 128 bytes, and contain no control characters.

`match: "score"` uses `review_threshold` and `action_threshold` to interpret each message's score. These are message decision thresholds, not model quality requirements. Reports include every finite score from 0 to 1, including scores below both thresholds. Scores below the review threshold allow, scores from the review threshold up to the action threshold review, and scores at or above the action threshold use the rule's action.

A supplied Rust `PolicyScorer` opts into named category scoring with `supports_model_categories()` and reports its identity through `model_version()`. It receives the original target text, the named category, and supplied context when requested. `Scheduler::with_factory` supports the same adapter. No model, category rating catalogue, or model loader is bundled in this release.

The default backend reports model rules with `MODEL_UNAVAILABLE`, and unimplemented deterministic categories with `DETECTOR_UNAVAILABLE`. Both produce `indeterminate` / `review` and list rule IDs in `coverage.unevaluated_rules`; they are not validation errors. Completed reports remain visible when another rule is unevaluated. Custom policy text and platform enforcement remain unsupported.

```sh
cargo run --locked --example runtime_chat -- examples/category-policy.json
python3 sdk/python/examples/send_flow.py /path/to/candidates.json examples/category-policy.json
node sdk/node/examples/send-flow.js /path/to/candidates.json examples/category-policy.json
```

The candidate file is your explicitly provisioned provider list. These reference hooks print available scores and unevaluated rules before the local send decision. Use an empty required-detector list for exploration; requiring a detector during discovery still requires that provider to actually implement it.

## Errors and boundaries

`E2EMError` exposes fixed codes; runtime errors do not include input text. Unavailable providers, timeouts and malformed replies hold the message for review. Cancellation is best effort. A returned result does not itself send or block a message; the application owns that action.

The service separates cooperating enrolled apps, but does not protect their credentials against hostile processes running as the same OS user. Named categories are accepted independently of model ratings. Custom policy text and platform profiles remain unsupported; malformed policies fail validation. Read the [contract](runtime/CONTRACT.md) and [security limits](runtime/SECURITY.md) for full bounds and semantics.
