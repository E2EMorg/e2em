# E2EM Dart desktop SDK

Assess chat messages locally from Dart or Flutter on Linux, macOS, and Windows.
The SDK connects to the installed runtime using authenticated Unix sockets or
Windows named pipes. It has no inference dependencies. Mobile and web are not
supported in this preview.

Requires Dart 3.5+. Install and enrol the runtime first using the
[setup guide](../../docs/INSTALL.md). For Daccord, open
`e2emd --setup --setup-app daccord` (or `e2em-setup.exe --setup-app daccord` on
Windows), then approve **Connect app** in the setup screen.

Add a dependency from your application to a checkout:

```yaml
dependencies:
  e2em_local:
    path: /path/to/e2em/sdk/dart
```

Or use a tagged Git dependency with `path: sdk/dart`, or extract the
`e2em-dart-sdk-0.1.2.tar.gz` release archive and point a path dependency at the
extracted directory. Publication to pub.dev is not enabled.

```dart
import 'package:e2em_local/e2em.dart';

final report = await assess('Hello there!', app: 'daccord');
print(report.scores);
print(report.unevaluated);
```

All 40 presets are selected by default. Use ordered context, a preset subset,
and custom policy text as needed:

```dart
await assess('That sounds good', app: 'daccord',
    context: ['Shall we meet tomorrow?']);
await assess('Hello there!', app: 'daccord',
    policies: ['identity.hate', 'abuse.threat']);
await assess('The launch is next week', app: 'daccord',
    customPolicies: ['Keep launch dates private.']);
```

These options accept `List<String>`. Custom policies add to the defaults; use
`policies: []` for custom-only assessment. `model` selects a registered model.

For an ongoing chat session, keep one client open:

```dart
final client = await Client.connect(app: 'daccord');
try {
  final report = await client.assess('Hello', policies: ['pii.email']);
  print(report.action);
} on E2EMError catch (error) {
  print(error.code); // Fixed code; action is always review.
} finally {
  client.close();
}
```

`Client.connect` and `assess` read installer-managed private settings for `app`
(`my-app` by default). `configPath` selects explicitly provisioned settings.
Unix loaders reject symlinks, wrong ownership, and group/other permissions.
Windows settings inherit installer-managed ACL protection; pipe names are
restricted to local `\\.\pipe\e2em-*` endpoints. Both transports verify the
provider's HMAC proof before accepting requests.

`Client.open` accepts explicit `socketPath`, `principal`, `secret`, and `provider`.
`Client.discover` tries explicitly provisioned `ProviderCandidate` objects in
native/project/embedded order with optional `requiredDetectors` and `pinned`.
It does not retry or switch providers after an assessment error. Do not retain
or log private settings.

Advanced integrations can use `client.assess(request)` with a JSON
`Map<String, dynamic>` and explicit message IDs/revisions, `validatePolicy`,
`capabilities`, `cancel`, `models`, and `installModel`. See the
[contract](../../docs/runtime/CONTRACT.md). Up to eight concurrent calls and
131,072-byte frames are allowed. Deadlines and protocol failures raise
`E2EMError` with `action == 'review'`.

Cancel an assessment using a `CancellationToken`; cancellation raises
`E2EMError('CANCELLED')` and requests runtime cancellation. Closing the client
also fails pending work and disposes transport resources.

Reports and `report.request` are immutable. Before sending, call
`report.appliesTo(currentRequest)` with your **current** message, context, policy,
and revision. An edit invalidates the result. Repeat this check after warning
confirmation. `allow` may proceed; `warn` requires explicit policy-permitted
confirmation. Hold `review`, errors, cancellations, stale results, and incomplete
coverage. The [send-flow example](example/send_flow.dart) illustrates this guard.

`report.scores` includes every reported finite score; `report.unevaluated` maps
unevaluated rule IDs to preset names where the request includes its policy.
`report.value` / `toJson()` contain the wire report, excluding the original
request. `utf16Span(text, start, end)` converts UTF-8 byte spans for Flutter's
UTF-16 text offsets and rejects code point splits.

## Development

```sh
cd sdk/dart
dart pub get --enforce-lockfile
dart analyze
dart test
cd ../..
cargo build --locked --bin e2emd
python3 scripts/check_dart_sdk.py --binary target/debug/e2emd
```

The last check starts a rules-only native daemon and uses shared conformance
fixtures. CI runs it on Linux, Windows, and both Intel/Apple Silicon macOS.
