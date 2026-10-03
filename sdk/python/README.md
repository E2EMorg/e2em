# E2EM Python SDK

A small local E2EM client for assessing chat messages before sending, for Python 3.11+. Assessment uses your installed runtime; the SDK has no inference dependencies.

```sh
python3 -m pip install ./sdk/python
# From a release download:
python3 -m pip install ./e2em_local-0.1.1-py3-none-any.whl
```

[Install the runtime](https://github.com/E2EMorg/e2em/blob/main/docs/INSTALL.md), enrol an app, and follow the [complete Python example](https://github.com/E2EMorg/e2em/blob/main/docs/SDK.md#python).

`Client` provides async `open`, `discover`, `capabilities`, `validate_policy`, `assess`, `cancel`, and `close`. Use `async with` for disposal. `BlockingClient` provides a synchronous adapter. `Assessment.applies_to(current_request)` verifies the current snapshot; `E2EMError` requires review. Revalidate policies after provider restart. Warnings require explicit user confirmation before the host acts.

Start with the [chat policy](https://github.com/E2EMorg/e2em/blob/main/examples/chat-policy.json). Preview 0.1 supports personal warnings for email addresses shared in chat messages (`pii.email`). Packages are distributed through [GitHub Releases](https://github.com/E2EMorg/e2em/releases); PyPI publication is not enabled. Licensed under [MIT](LICENSE).
