# E2EM Node / TypeScript SDK

A local E2EM client for assessing chat messages before sending, for Node.js 22+, with generated TypeScript contract types and no inference dependencies.

```sh
npm install ./sdk/node
# From a release download:
npm install ./e2em-local-0.1.3.tgz
```

[Install the runtime](https://github.com/E2EMorg/e2em/blob/main/docs/INSTALL.md), complete runtime setup, and follow the [complete JavaScript example](https://github.com/E2EMorg/e2em/blob/main/docs/SDK.md#node-and-typescript).

```javascript
import {assess} from '@e2em/local';

const report = await assess('Hello there!', {context: ['Earlier message']});
console.log(report.scores, report.unevaluated);
```

`Client.connect()` registers new apps and loads their local settings automatically without asking permission. `assess(message, {context, policies, customPolicies})` is the simple API. `Client` also provides Promise-based `open`, `discover`, `capabilities`, `validatePolicy`, `assess`, and `cancel`. Call `close()` on disposal. Assessment results are frozen; `appliesTo(currentRequest)` checks the current snapshot. Errors require review; complete warning results are advisory and require no confirmation. Revalidate policies after a provider restart.

The `./browser` entry point defines an explicit bridge interface; it does not provide Node IPC or a browser extension transport.

Send a message with `assess(message)`, optionally include context, and use all 40 built-in presets by default. Preset subsets and custom policy strings are optional; no policy files or manual credential handling are needed in code. The service uses Gandalf by default, scores custom policy text, and supports selection of registered custom models. Missing required context or token truncation stays visible as incomplete coverage. [Model setup and updates](../../docs/runtime/MODELS.md). Packages are distributed through [GitHub Releases](https://github.com/E2EMorg/e2em/releases); npm registry publication is not enabled. Licensed under [MIT](LICENSE).
