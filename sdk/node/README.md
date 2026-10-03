# E2EM Node / TypeScript SDK

A local E2EM client for assessing chat messages before sending, for Node.js 22+, with generated TypeScript contract types and no inference dependencies.

```sh
npm install ./sdk/node
# From a release download:
npm install ./e2em-local-0.1.1.tgz
```

[Install the runtime](https://github.com/E2EMorg/e2em/blob/main/docs/INSTALL.md), enrol an app, and follow the [complete JavaScript example](https://github.com/E2EMorg/e2em/blob/main/docs/SDK.md#node-and-typescript).

`Client` provides Promise-based `open`, `discover`, `capabilities`, `validatePolicy`, `assess`, and `cancel`. Call `close()` on disposal. Assessment results are frozen; `appliesTo(currentRequest)` checks the current snapshot. Errors require review; warnings require explicit user confirmation. Revalidate policies after a provider restart.

The `./browser` entry point defines an explicit bridge interface; it does not provide Node IPC or a browser extension transport.

Start with the [chat policy](https://github.com/E2EMorg/e2em/blob/main/examples/chat-policy.json). All named categories are accepted for reporting without a model rating gate. The bundled backend evaluates email addresses (`pii.email`); unavailable checks appear as unevaluated in an indeterminate report. Custom policy text remains unsupported. Packages are distributed through [GitHub Releases](https://github.com/E2EMorg/e2em/releases); npm registry publication is not enabled. Licensed under [MIT](LICENSE).
