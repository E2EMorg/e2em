# Desktop developer runtime and SDKs

Build `cargo build --locked --workspace --all-features`. `e2emd` is feature-gated
by `runtime-service`, with Linux Unix sockets, a macOS launchd prototype and a
Windows named-pipe prototype. It serves
only personal email warnings, without Python workers or model downloads.

MSI, PKG, DEB and RPM build/lifecycle workflows are described in
[PACKAGING.md](PACKAGING.md). Packages install the executable and user setup tools;
enrolment/startup remain explicit. Use `--use-packaged-binary` on Unix or
`-UsePackagedBinary` on Windows so user setup references the package-owned
executable and upgrades preserve credentials. Existing source-copy installs keep
their previous behavior. Release artifacts are unsigned developer previews; consult each release's attached
native reports and workflow results for platform qualification.

Install explicitly (no administrator permission):

```sh
python3 scripts/install_runtime.py install --binary /path/to/e2emd
python3 scripts/install_runtime.py enrol reference-python
python3 scripts/install_runtime.py enrol reference-node
systemctl --user daemon-reload
systemctl --user enable --now e2emd
```

The installer copies the binary to `~/.local/bin`, writes a user systemd unit,
creates private grant/credential files under `~/.config/e2em`, and prints no secrets.
Enrolment chooses an application slug and creates an explicit personal grant.
Re-enrolment rotates it. The unit creates a private `%t/e2em` endpoint directory.
Install refuses to overwrite an existing installation. Normal assessment needs no
root access. For a foreground test, make an owned mode-0700 directory and start:

```sh
e2emd --socket /private/directory/runtime.sock --grants /private/directory/grants.json
```

Grants format: `{"provider":"project-instance","grants":[{"principal":"app","uid":1000,"secret":"<64 random hex characters>"}]}`.
The UID is the actual local account, not a machine-specific default. Generate
secrets with the installer. An enrolment file gives deployment credentials;
applications load these before assessment, and must not accept them from a page.

Revoke with `python3 scripts/install_runtime.py revoke APP`. To remove, stop and
disable the unit first, then run `python3 scripts/install_runtime.py uninstall`
and reload the user service manager. The script removes only its managed files,
not arbitrary application drafts. Offline installation is supported. No inference
assets are provisioned during assessment.

## macOS user agent prototype

Build on macOS with the same Cargo command, then explicitly install and enrol:

```sh
python3 scripts/install_macos_runtime.py install --binary /path/to/e2emd
python3 scripts/install_macos_runtime.py enrol reference-python
launchctl bootstrap gui/$(id -u) "$HOME/Library/LaunchAgents/org.e2em.runtime.plist"
```

The installer writes a mode-0600 LaunchAgent with separate argument strings,
private grants under `~/.config/e2em`, and a private socket directory under
`~/Library/Caches/e2em`. It rejects socket paths at Darwin's 104-byte limit before
writing files. Installation does not start the agent. launchd starts it at login
and restarts unsuccessful exits; no periodic launch interval is configured.
Use `launchctl bootout gui/$(id -u)/org.e2em.runtime` before uninstalling with
`python3 scripts/install_macos_runtime.py uninstall`. Enrolment rotation/revocation
use the same explicit commands and grant format as Linux. Sandboxed apps and
entitlements remain unqualified; this prototype targets unsandboxed user apps.

Native qualification (requires a macOS GUI login, Python and Node 22+) is prepared:

```sh
cargo test --locked --workspace --all-features
python3 scripts/check_runtime_macos.py --binary /path/to/e2emd --output /tmp/e2em-macos.json
```

The checker creates and removes a transient launchd agent, runs Python and Node
fixtures together, compares the shared provider, and records three idle-RSS cycles.
It refuses to qualify a cross-build as a native run. The context experiment also
compiles a static-ABI C probe, runs it as an ordinary user executable and as an
ad-hoc signed App Sandbox bundle without exceptions, records actual sandbox/IPC
state, and requires the embedded rules warning to work in both. It records whether
the socket is denied or merely reachable; socket reachability alone never qualifies
shared authentication/enrolment. It removes its own unique sandbox container.
Native execution is still pending. No App Group, notarization, App Store or
universal sandbox bypass is claimed.

## Windows user pipe prototype

Build on Windows with `cargo build --locked --workspace --features runtime-service`.
From a normal user PowerShell session:

```powershell
./scripts/install_windows_runtime.ps1 -Action install -Binary ./target/debug/e2emd.exe
./scripts/install_windows_runtime.ps1 -Action enrol -Principal reference-python
```

The installer copies the binary under `%LOCALAPPDATA%\E2EM`, protects its directory
and managed files with current-user/SYSTEM ACLs, and prints the explicit foreground
startup command. Installation does not register a machine service or start a
process. The local pipe is `\\.\pipe\e2em-<current-user-SID>`; remote pipe names
are rejected by the daemon and both SDKs. Windows grant entries use `sid` instead
of Unix `uid`. The daemon checks the connected process token's user SID and then
the shared explicit application grant. It continuously retains a server instance,
refuses a competing first instance, and rechecks private grant ownership/ACLs on
every operation. Stop with Ctrl+C before `-Action uninstall`; `-Action revoke
-Principal APP` and re-enrolment remove/rotate grants. Unmanaged files are preserved.

The Python client uses the standard Windows Proactor event loop; Node uses its
built-in pipe transport. Existing `socket_path` / `socketPath` credential fields
carry the local pipe name. The APIs and mutual provider proof remain identical.
Unpackaged user applications are the prototype scope; AppContainer/packaged app
broker access is not qualified. Windows 10+ is the build target; a tested minimum
OS version will be published only after native qualification.

```powershell
cargo test --locked --workspace --features runtime-service --test runtime_windows
python scripts/check_runtime_windows.py --binary ./target/debug/e2emd.exe --output e2em-windows.json
```

The native checker uses a temporary private user installation, runs both SDKs
against one provider, checks idle eviction and revocation, stops the process, and
uninstalls it. It also checks secret rotation, overwrite/live-uninstall refusal,
unmanaged-file preservation, and rejects a broadened grant ACL both in the daemon
and enrolment tool. The installer validates existing private file ownership and
ACLs before replacement, because [ReplaceFile preserves the destination DACL](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-replacefilea).
It refuses non-Windows hosts. The installer has passed PowerShell
syntax validation on Linux; its Windows ACL/lifecycle execution still needs a
native test host. Win32 unsafe calls are confined to `crates/e2em-platform`; the
main runtime retains its unsafe-code prohibition.

## Memory pressure

On Linux the service registers a kernel PSI trigger for 300ms of partial memory
stalls within a two-second window. It awaits priority readiness events instead
of periodically reading pressure counters. This window supports unprivileged
monitors; see the [kernel PSI documentation](https://docs.kernel.org/accounting/psi.html).
Notifications use the scheduler's existing eviction path: assets remain valid
until queued/active work completes, and new admissions stop during deferred
eviction. Idle clients and capability calls never load the scorer.

When PSI is absent or inaccessible, startup reports that automatic monitoring is
unavailable and continues with idle eviction and the explicit `SIGUSR1` hook.
A failed event source is disabled with a diagnostic rather than retried in a
loop. Shutdown closes the trigger descriptor and deregisters the monitor.
This qualifies the Linux notification plumbing only; the preview loads no model
weights or accelerator resources.

On Windows, a kernel resource-notification wait requests eviction at low memory.
It waits for recovery before rearming, so a sustained low-memory event cannot
spin or repeatedly reload assets. Shutdown signals a separate event and joins
the waiting thread. On macOS, a serial dispatch memory-pressure source delivers
warning/critical callbacks through a bounded channel; its cancellation handler
reclaims the callback context after the final event. These adapters pass cross-
target checks. The Windows synthetic event tests also execute under Wine; real
OS pressure delivery and the macOS callback tests still require native hosts.
The APIs are documented by [Microsoft](https://learn.microsoft.com/en-us/windows/win32/api/memoryapi/nf-memoryapi-creatememoryresourcenotification)
and [Apple](https://developer.apple.com/documentation/dispatch/dispatch_source_type_memorypressure).

## Disposable worker and resource evidence

`runtime::process::NativeProcessScorer` is an explicit factory option for trusted,
provisioned native scorers whose allocators retain memory. It uses bounded private
stdio frames, a versioned startup handshake, serialized calls and configurable
startup/response timeouts (positive, at most five seconds). Eviction and protocol,
startup, score or timeout failures kill and reap the child and join its reader.
Invalid probabilities and worker errors produce backend failure/review. Executable
paths and arguments come from trusted provisioning, never application messages;
there is no download or implicit process fallback in `e2emd`. Workers must not
spawn descendants retaining their stdio pipes. Backend-specific model/tokenizer/
accelerator qualification remains required when a native inference backend ships.

The diagnostic `e2em-resource-worker` is gated by `runtime-probes`. It holds
page-touched synthetic CPU buffers and scores the same rules fixture; it is not
a contextual model. The 64-MiB in-process probe reproduced allocator retention:
the third eviction left about 58 MiB resident. The disposable probe reaped every
worker and returned the broker to about 9 MiB across idle and pressure cycles,
with one load per eight concurrent requests and retained policy references.
Both results are published in `evidence/linux-cpu-buffer-inprocess-preview.json`
and `evidence/linux-cpu-buffer-disposable-preview.json`.

```sh
cargo build --locked --features runtime-probes --bin e2em-resource-worker
cargo run --locked --example runtime_resource_probe -- --output /tmp/reclaim.json --disposable-worker /absolute/path/to/e2em-resource-worker
```

The probe without `--disposable-worker` records the in-process comparison and
returns failure if allocator retention exceeds its documented residual bound.
Its test/measurement sampling does not add polling to the runtime.

`evidence/linux-rules-energy-preview.json` includes real RAPL energy counters,
all-thread context switches/scheduled timeslices, CPU ticks, unloaded RSS and
repeated cold/warm cycles. Three connected-unloaded five-second windows measured
zero broker CPU ticks and scheduled timeslices, with roughly 10 MiB residual RSS.
RAPL measures whole CPU domains on a shared host: unrelated work and measurement
overhead are included, overlapping domains are not summed, and no per-process
power attribution is claimed. Scheduler counters are wakeup proxies, not hardware
interrupt tracing. Read access uses an explicit existing offline Node container
with only the powercap directory mounted read-only; assessment stays unprivileged.

```sh
python3 scripts/measure_runtime.py --binary /path/to/e2emd --output /tmp/energy.json --cycles 3 --idle-window-seconds 5 --energy-via-container node:22-bookworm
```

Use `--energy` instead when the current user can directly read RAPL counters.
The container image is never pulled automatically; cleanup removes its reader.

## Client APIs

Python package: `pip install ./sdk/python` (Python 3.11+, standard library at run
time). `Client.open(...)`, or `Client.discover(candidates, required_detectors,
pinned=None)`, then await `capabilities()`, `validate_policy()`, `assess()` and
`cancel()`. Use `async with` or close explicitly. `BlockingClient` owns a dedicated
loop for optional blocking integration. Assessment returns defensive-copy values
and an `applies_to(current_request)` snapshot check. `E2EMError` carries a fixed
code and review action. Never translate it into an automatic send.

Node package: `npm install ./sdk/node` (Node 22+). The Promise API has `open`,
`discover`, `capabilities`, `validatePolicy`, `assess` (optional AbortSignal),
`cancel` and `close`. TypeScript types are generated; runtime replies are schema-
checked. Results are recursively frozen and include `appliesTo(currentRequest)`.
`utf16Span` in both SDKs converts original bytes for UTF-16 UIs.

The SDKs reject incompatible/unavailable providers, incompatible required coverage,
wrong provider proofs, malformed results and stale response identities. Reconnect
never reuses policy references. Native-provider migration is demonstrated with a
configured mock native candidate using the same local contract; no real OS provider
or automatic system-wide provider enumeration is claimed. Candidate lists are
provisioning configuration, never inference-time fallback downloads.

`examples/runtime_chat.rs` is a local Rust chat. SDK outgoing hooks are in
`sdk/python/examples/send_flow.py` and `sdk/node/examples/send-flow.js`. They load
a provisioned candidate array and policy JSON, check current snapshots and require
explicit warning continuation. They do not access real accounts or send messages.
Cancellation calls are shown by shared SDK tests and can be used when a draft is
superseded. Incoming/display/notification accessibility integration is a later gate.

| Platform/client | Current evidence |
| --- | --- |
| Linux Rust/C/C++, Python 3.12, Node 22 | Executed embedded and live service fixtures; two independent apps share one worker |
| Linux Python 3.11+ / Node 22+ | Declared API floors; GitHub CI pins these versions (Forgejo uses Debian Python 3.11) |
| Windows GNU embedded C ABI / pipe prototype | Cross-build; C ABI and pipe authentication/fixture/revocation/lifecycle compatibility under Wine; native security/install/packaged-app qualification remains #23 |
| macOS Unix agent prototype | Cross-check and installer tests pass; native launchd/peer/sandbox qualification remains #24 |
| Sandboxed Linux/macOS apps | No shared-service support promised; use embedded core where permitted |
| Browser | Separate explicit extension bridge interface; no Node IPC in browser or browser inference |

## Verification and remaining gates

Run the repository Cargo/Python checks plus Node tests and generated type checks:

```sh
cargo build --locked --workspace --all-features
cargo test --locked --workspace --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
python3 -m compileall -q scripts tests sdk/python
python3 -m unittest discover -s tests -p 'test_*.py'
node --test sdk/node/test.mjs
npx --yes --package typescript@5.9.3 tsc --strict --noEmit --module nodenext --moduleResolution nodenext --target ES2022 sdk/node/typecheck.ts
```

The Rust live-service test invokes Python and Node, so Node must be installed for
`--all-features` testing on Linux. C/C++ foreign conformance is run after building
with `E2EM_FFI_LIBRARY=/path/to/libe2em_ffi.so E2EM_ASAN=1 python3 -m unittest
discover -s tests -p test_runtime_ffi.py -v`. In ordinary Python discovery, live
service/FFI tests are explicitly skipped when those build paths are absent; Rust
integration and the CI foreign-client step execute them.

Linux sanitizer clients use `-fno-pie -no-pie` to avoid GCC ASan startup/shadow
mapping collisions under container ASLR; AddressSanitizer and UBSan remain
enabled. In the runner image an empty PIE program failed 13/40 starts, while
the same non-PIE program passed 40/40. The service and production library do
not change their executable layout for this test workaround.

Linux measurements are in `evidence/linux-rules-preview.json` and the PSI-enabled
`evidence/linux-rules-psi-preview.json`, and the lazy-heuristic update
`evidence/linux-rules-lazy-preview.json`, reproducible with
`scripts/measure_runtime.py`. They include IPC/SDK time, connected idle eviction,
residual RSS across cycles and context-switch observations. They contain no model
performance or energy claim. Runtime CPU-allocation disposal and energy evidence are now published separately
above; native inference model/tokenizer/accelerator qualification remains a backend
release gate, and these rules/synthetic-buffer reports do not qualify it. Linux PSI registration was executed as an unprivileged user; priority
notification sleep/rearm/cancellation tests and safe scheduler eviction tests pass.
No host-wide memory exhaustion experiment was performed. Issues
#23/#24 require native Windows/macOS test hosts before their security, installation
and sandbox acceptance can be closed. Signed model activation/update tests remain
#34; policy versions in the preview remain immutable across updates.

The available local Forgejo runner labels are `docker` and `soe-linux`, both backed
by Linux Node 22/bookworm containers. `.forgejo/workflows/ci.yml` targets `docker`
and installs native build prerequisites; `.github/workflows/ci.yml` retains the
GitHub-hosted workflow. No Windows/macOS runner was discovered in the configured
local inventory. The only configured SSH host failed strict host-key verification;
its changed key was not accepted as evidence of another test machine. Windows
GNU and macOS x86_64 cross-checks pass, but do not replace native platform runs.
The transient per-user systemd lifecycle check is reproducible with
`python3 scripts/check_runtime_systemd.py /path/to/e2emd` and removes its own unit
and endpoint after testing.

Wine passes the named-pipe request/lifecycle fixture test but fails
`first_instance_rejects_namespace_takeover`: Wine permits a second first-instance
handle. This test remains mandatory on native Windows; it is not weakened or
counted as a pass. See `evidence/windows-pipe-wine-preview.json` for the bounded
compatibility result. Linux live tests also exercise both SDK Windows adapter
branches over a redirected fixture transport; that does not qualify IOCP or ACLs.

A prepared GitHub native workflow (`.github/workflows/runtime-native.yml`) runs
these Windows/macOS tests and publishes native checker reports when suitable
hosted runners are available. This repository currently pushes to local Forgejo;
adding the workflow does not constitute a native run. For local Wine reproduction:

```sh
cargo test --locked --workspace --features runtime-service --target x86_64-pc-windows-gnu --test runtime_windows --no-run
python3 scripts/check_runtime_pipe_wine.py /path/to/runtime_windows-test.exe --output /tmp/e2em-pipe-wine.json
```
