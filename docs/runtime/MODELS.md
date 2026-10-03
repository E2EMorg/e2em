# Runtime models

E2EM 0.1.2 uses [Gandalf](https://huggingface.co/krazyjakee/gandalf) by default. Normal installers include a native CPU worker and ONNX Runtime; per-user setup downloads and verifies the model before reporting success. Offline installers include the same verified deployment package, so setup can import it without a network connection. Assessment never downloads assets or sends messages over the network. Neither a GPU nor Python inference dependencies are required.

Gandalf 0.0.1 is pinned to Hugging Face revision `7477acb03721e7c59cce1e12f2163c0ad33cfbfa`. The original weights have SHA-256 `916370da013a428a8ed3e00791049d12b3b3fd5836969d8b0492bc5de372d5c8`. The deployment uses a FP32 ONNX export, compared against the original PyTorch checkpoint at multiple batch sizes and token lengths. The owner authorized MIT distribution; the package preserves upstream attribution, base-model licence and source provenance.

All 40 default policies remain available. Email uses its exact detector. Other defaults use model scores, including a model fallback for categories without a deterministic detector. `spam.repeat` requires previous conversation; without it that rule is explicitly unevaluated. Gandalf's 26 published policy thresholds apply to matching wordings. Other categories use the runtime defaults (review 0.4, warning 0.7). Custom policy text uses 0.5 unless a matching package threshold is provided. Supporting a wording does not establish model accuracy for that wording; per-category quality remains a separate evaluation task.

## Provisioning and updates

The owner-private store sits beside `grants.json`: `models.json` records aliases, trusted worker/library paths, the default model and current/previous packages; `models/` contains verified assets. Setup installs the backend through the OS package manager and imports assets into this store. Grants and secrets stay separate from model packages.

The default deployment descriptor is Ed25519 signed with the public key in `src/runtime/gandalf-public-key.hex`. Every asset has an exact size and SHA-256. Downloads use HTTPS, retain interrupted partial files, and resume when the server supports Range. A candidate must verify and pass a native startup/scoring smoke check before atomic activation. Failed candidates leave the working package selected. The previous package remains available for rollback.

Enabled model updates check the latest stable E2EM release's `gandalf-model.json` every six hours, after at least 60 seconds without requests. They accept only a newer semantic model version with a valid release signature. Model updates do not require a daemon version change. Raw Hugging Face training checkpoints are converted, compared and signed by the release pipeline before they become runtime updates; the runtime never runs downloaded repository code. Custom sources update only when the owner opts in.

`--offline` disables all network access, including daemon and model updates. Setup accepts `--offline` (Windows `-Offline`). Choosing an offline installer alone still permits future verified updates when online. `--no-auto-update` / `-NoAutoUpdate` pins both updates during setup.

Inspect, explicitly check, select or roll back a model:

```sh
e2emd --grants ~/.config/e2em/grants.json --model-status
e2emd --grants ~/.config/e2em/grants.json --model-check-now
e2emd --grants ~/.config/e2em/grants.json --model-use custom
e2emd --grants ~/.config/e2em/grants.json --model-rollback gandalf
```

Rollback disables automatic updates for that alias. Reinstall with automatic updates enabled to resume. If initial setup is interrupted, retain the private store and rerun `e2emd --grants PATH --model-install gandalf`; completed downloads are reused. Start the installed service only after this command succeeds. For an offline retry supply the local package directory instead.

## Custom models and SDKs

Register a compatible deployment manifest URL, a Hugging Face repository publishing `e2em-model.json`, or a local package directory:

```sh
e2emd --grants ~/.config/e2em/grants.json --model-install https://example.org/model.json --model-name custom --model-no-update
```

Unregistered aliases return `MODEL_UNAVAILABLE` and later requests can still use Gandalf. At most eight aliases are registered. An application needs an explicit model-management grant to install packages through the SDK: enrol with `--model-management` (Windows `-ModelManagement`). Ordinary enrolled apps can inspect and select already registered models.

```python
client = await Client.connect("my-app", model="custom")
await client.install_model("https://example.org/model.json", name="custom")
report = await client.assess("Hello", model="gandalf")
```

```js
const client = await Client.connect({app:'my-app', model:'custom'});
await client.installModel('https://example.org/model.json', {name:'custom'});
const report = await client.assess('Hello', {model:'gandalf'});
```

Rust uses `Scheduler::with_models(idle, models::Manager::new(config_path))` and `CheckOptions.model`; C uses `e2em_open_models(1, config_path_bytes, length)` and the request's `options.model`. The existing embedded `Engine::default()` and C `e2em_open(1)` remain explicit detector-only entry points. The daemon requires provisioned models by default; `--rules-only` is a diagnostic mode.

## Deployment format and limits

The descriptor contains `manifest` and an optional hex Ed25519 `signature`. Its format is `e2em-onnx-policy-cross-encoder-v1`; metadata specifies model ID/version, source revision, licence, 512-token input limit, retained tail tokens and `evidence_format: target-first`. `files` maps flat filenames to HTTPS URLs, byte sizes and SHA-256 values. Mandatory files are `model.onnx` and `tokenizer.json`; optional `preset.json` provides policy wordings and action/review thresholds. Use the published `gandalf-model.json` as the complete example. Explicit custom sources may be unsigned; the owner controls their trust. Downloaded manifests cannot supply executable or shared-library paths.

Compatible ONNX graphs accept int64 `input_ids` and `attention_mask`, both `[batch, sequence]`, and return float32 `logits`, `[batch, 1]`. Scores are sigmoid violation probabilities. A Hugging Face training model or arbitrary weights URL needs export and deployment metadata before it is compatible.

The tokenizer encodes policy first and message/context second. With context the evidence is `Message:\n{message}\n\nContext:\n{context}`; without it the evidence is just the message. The policy is retained, evidence fills the remaining 512-token budget with its beginning and up to 64 trailing tokens. Any truncation produces incomplete coverage, `MODEL_TOKEN_LIMIT`, and an indeterminate review outcome. Two CPU threads and microbatches of eight bound inference. The shared scheduler owns at most one model process, switches packages when selected, and kills/reaps it on idle eviction, pressure, cancellation or failure. Deadlines default to 15 seconds and permit 1–30,000 ms.

ONNX parity, native SDK checks and installer provisioning are release gates. They demonstrate implementation behavior, not model quality or operating-system sandbox isolation. Release installers are unsigned; the signed model descriptor does not change that status.
