# E2EM Python SDK

A small local E2EM client for assessing chat messages before sending, for Python 3.11+. Assessment uses your installed runtime; the SDK has no inference dependencies.

```sh
python3 -m pip install ./sdk/python
# From a release download:
python3 -m pip install ./e2em_local-0.1.2-py3-none-any.whl
```

[Install the runtime](https://github.com/E2EMorg/e2em/blob/main/docs/INSTALL.md), enrol an app, and follow the [complete Python example](https://github.com/E2EMorg/e2em/blob/main/docs/SDK.md#python).

```python
from e2em import assess

report = assess("Hello there!", context=["Earlier message"])
print(report.scores, report.unevaluated)
```

`assess(message, context=..., policies=..., custom_policies=...)` is the simple API. `Client.connect()` and `BlockingClient()` load your enrolled app settings automatically. `Client` also provides async `open`, `discover`, `capabilities`, `validate_policy`, `assess`, `cancel`, and `close`. Use `async with` for disposal. `BlockingClient` provides a synchronous adapter. `Assessment.applies_to(current_request)` verifies the current snapshot; `E2EMError` requires review. Revalidate policies after provider restart. Warnings require explicit user confirmation before the host acts.

Send a message with `assess(message)`, optionally include context, and use all 40 built-in presets by default. Preset subsets and custom policy strings are optional; no policy files or manual credential handling are needed in code. The service uses Gandalf by default, scores custom policy text, and supports selection of registered custom models. Missing required context or token truncation stays visible as incomplete coverage. [Model setup and updates](../../docs/runtime/MODELS.md). Packages are distributed through [GitHub Releases](https://github.com/E2EMorg/e2em/releases); PyPI publication is not enabled. Licensed under [MIT](LICENSE).
