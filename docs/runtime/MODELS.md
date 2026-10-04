# Runtime models

E2EM 0.1.2 uses [Gandalf](https://huggingface.co/krazyjakee/gandalf) by default. Normal installers include a native worker and ONNX Runtime with platform acceleration; per-user setup downloads and verifies the model before reporting success. Offline installers include the same verified deployment package, so setup can import it without a network connection. Existing installations upgraded from 0.1.1 provision the model before the first service start if their model store is missing. This preserves app grants and uses the installed native backend. Assessment never downloads assets or sends messages over the network. A compatible accelerator is preferred automatically; CPU remains available. Python inference dependencies are not required on the installed machine.

Gandalf 0.0.1 is pinned to Hugging Face revision `7477acb03721e7c59cce1e12f2163c0ad33cfbfa`. The original weights have SHA-256 `916370da013a428a8ed3e00791049d12b3b3fd5836969d8b0492bc5de372d5c8`. The deployment uses a FP32 ONNX export, compared against the original PyTorch checkpoint at multiple batch sizes and token lengths. The owner authorized MIT distribution; the package preserves upstream attribution, base-model licence and source provenance.

All 40 default policies remain available. Email uses its exact detector. Other defaults use model scores, including a model fallback for categories without a deterministic detector. `spam.repeat` requires previous conversation; without it that rule is explicitly unevaluated. Gandalf's 26 published policy thresholds apply to matching wordings. Other categories use the runtime defaults (review 0.4, warning 0.7). Custom policy text uses 0.5 unless a matching package threshold is provided. Supporting a wording does not establish model accuracy for that wording; per-category quality remains a separate evaluation task.

## Provisioning and updates

The owner-private store sits beside `grants.json`: `models.json` records aliases, trusted worker/library paths, the default model and current/previous packages; `models/` contains verified assets. Setup installs the backend through the OS package manager and imports assets into this store. Grants and secrets stay separate from model packages.

The default deployment descriptor is Ed25519 signed with the public key in `src/runtime/gandalf-public-key.hex`. Every asset has an exact size and SHA-256. Downloads use HTTPS, retain interrupted partial files, and resume when the server supports Range. A candidate must verify and pass a native startup/scoring smoke check before atomic activation. Failed candidates leave the working package selected. The previous package remains available for rollback.

Enabled model updates check the latest stable E2EM release's `gandalf-model.json` every six hours, after at least 60 seconds without requests. They accept only a newer semantic model version with a valid release signature. Model updates do not require a daemon version change. Raw Hugging Face training checkpoints are converted, compared and signed by the release pipeline before they become runtime updates; the runtime never runs downloaded repository code. Custom sources update only when the owner opts in.

`--offline` disables all network access, including daemon and model updates. Guided setup has an **Offline installation** option; unattended setup accepts
`--setup-headless --offline`. Legacy scripts accept `--offline` (Windows `-Offline`). Choosing an offline installer alone still permits future verified updates when online. `--no-auto-update` / `-NoAutoUpdate` pins both updates during setup.

Inspect, explicitly check, select or roll back a model:

```sh
e2emd --grants ~/.config/e2em/grants.json --model-status
e2emd --grants ~/.config/e2em/grants.json --model-check-now
e2emd --grants ~/.config/e2em/grants.json --model-use custom
e2emd --grants ~/.config/e2em/grants.json --model-rollback gandalf
```

Rollback disables automatic updates for that alias. Reinstall with automatic updates enabled to resume. If guided setup is interrupted, reopen **E2EM Setup** and choose **Try again**.
For legacy command-line setup, retain the private store and rerun `e2emd --grants PATH --model-install gandalf`; completed downloads are reused. Start the installed service only after this command succeeds. For an offline retry supply the local package directory instead.

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

The tokenizer encodes policy first and message/context second. With context the evidence is `Message:\n{message}\n\nContext:\n{context}`; without it the evidence is just the message. The policy is retained, evidence fills the remaining 512-token budget with its beginning and up to 64 trailing tokens. Any truncation produces incomplete coverage, `MODEL_TOKEN_LIMIT`, and an indeterminate review outcome. At most two CPU threads (one for a single-CPU allocation) and microbatches of eight bound inference. The shared scheduler owns at most one model process, switches packages when selected, and kills/reaps it on idle eviction, pressure, cancellation or failure. Deadlines default to 15 seconds and permit 1–30,000 ms.

## Hardware admission

The worker defaults to automatic acceleration: CUDA on Linux NVIDIA hardware, DirectML on Windows GPUs, and CoreML on macOS (GPU/Neural Engine where the graph permits). It first checks provider availability, then registers the accelerator with errors enabled, loads the model and scores a fixed startup probe before announcing readiness. Failed initialization falls back to a fresh CPU session. If GPU execution fails later, automatic mode releases that session, rechecks CPU admission and retries the batch on CPU once. Unsupported graph operators can also run on CPU within an accelerated session. Tokenization and deterministic detectors remain CPU work.

Linux packages contain CUDA 12/cuDNN 9 native dependencies and their licence notices; they still require a compatible NVIDIA driver. Windows packages contain DirectML and macOS packages require the CoreML provider. These platform payloads are validated during staging. Larger CUDA payloads include no Python runtime. Owner-provisioned Linux ONNX Runtime builds may additionally use MIGraphX, ROCm, OpenVINO GPU or WebGPU; these providers are tried when the installed library advertises them. The default Linux package does not supply those additional vendor runtimes. See the upstream [CUDA](https://onnxruntime.ai/docs/execution-providers/CUDA-ExecutionProvider.html), [DirectML](https://onnxruntime.ai/docs/execution-providers/DirectML-ExecutionProvider.html) and [CoreML](https://onnxruntime.ai/docs/execution-providers/CoreML-ExecutionProvider.html) requirements.

Before downloading a candidate's assets or starting a model worker, the runtime checks available physical memory and the process's CPU allocation. Linux also applies the current process's cgroup memory limit and ancestor headroom when reported by [sysinfo](https://docs.rs/sysinfo/0.39.6/sysinfo/struct.Process.html#method.cgroup_limits). Swap is excluded. Startup requires four times the ONNX file size plus 1 GiB of available RAM: approximately 2.06 GiB for the current 284,315,095-byte Gandalf export. This is a conservative admission estimate for session/graph allocations and inference/OS headroom, not a measured peak or a guarantee of acceptable latency on every CPU. Unknown CPU or physical memory capacity refuses startup.

The worker repeats the startup check when invoked directly, and checks for 1 GiB of remaining host headroom before each inference microbatch. CUDA checks free VRAM for visible devices against the same estimate, bounds its device arena, and checks for 256 MiB of remaining device memory before each batch. Driver device ordinals respect `CUDA_VISIBLE_DEVICES`. Insufficient resources fail installation with an explicit RAM diagnostic. During assessment, rejected startup returns `MODEL_UNAVAILABLE`; an unrecovered resource failure inside an inference batch returns `INTERNAL_ERROR`. Both produce a review outcome. Idle/pressure eviction and request deadlines remain active. Checks take snapshots at admission and cannot reserve RAM/VRAM against other processes. Tested minimum CPU performance and native Windows/macOS GPU qualification remain separate release work.

Inspect actual worker selection with a fixed scoring probe (paths must be absolute):

```sh
e2em-inference --model /absolute/model/package --library /absolute/native/libonnxruntime.so --device-status
```

`--device auto` is the default; `--device cpu` skips GPU attempts; `--device gpu` requires accelerator initialization and does not rebuild on CPU after a failure. ONNX may still assign unsupported operators to CPU. On Linux, direct invocation needs the staged native directory on `LD_LIBRARY_PATH` for CUDA dependencies; the daemon sets this for its worker. Add `--profile /absolute/profile-prefix` to the diagnostic command to record actual ONNX node execution. `scripts/check_inference_devices.py` compares CPU and automatic scores, checks profiling evidence, and verifies CPU fallback with no visible CUDA devices. `--require-gpu` makes accelerator execution mandatory for that checker. The [Linux GPU report](evidence/linux-gpu-preview.json) verifies actual CUDA node execution and CPU fallback for a fixed Gandalf probe, without qualifying throughput or full-length GPU memory.

ONNX parity, native SDK checks and installer provisioning are release gates. They demonstrate implementation behavior, not model quality or operating-system sandbox isolation. Release installers are unsigned; the signed model descriptor does not change that status.

Intel Mac builds use ONNX Runtime 1.23.2 (its last published Intel wheel); other builds use 1.24.4. Every native build compares its scores with PyTorch references before release.
