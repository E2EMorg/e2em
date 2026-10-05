# Runtime packages

The build produces `e2em-runtime-VERSION-TARGET.{msi,pkg,deb,rpm}` plus SHA-256
checksums and JSON metadata. Version comes from `Cargo.toml`. These are unsigned
release artifacts. Normal installers include the native inference worker and platform ONNX Runtime providers with CPU fallback. Linux includes CUDA dependencies, Windows includes DirectML, and macOS includes CoreML. Offline variants (`e2em-runtime-VERSION-offline-TARGET`) also include verified Gandalf assets. No grants or credentials are included. Runtime inference needs no Python;
Python is only required for optional legacy setup scripts. No package script starts
a root service or enrols applications automatically.

## Package ownership and per-user setup

Linux DEB/RPM installs `/usr/bin/e2emd`, setup/docs in `/usr/share/e2em`, and
**E2EM Setup** in the applications menu. It starts the systemd user service.
macOS PKG installs `/usr/local/libexec/e2em/e2emd`, setup/docs in
`/usr/local/share/e2em`, and `/Applications/E2EM Setup.app`. Its finish screen
directs users to that app; setup registers a LaunchAgent. Windows MSI installs
under `%LOCALAPPDATA%\E2EM Runtime`, creates a Start menu **E2EM Setup** shortcut,
and offers to open setup when installation finishes. Quiet MSI installs do not
launch setup. Windows setup starts a hidden background process and registers an
HKCU login entry, without administrator rights or a foreground console.

On every platform, open **E2EM Setup** and choose **Set up E2EM**. The same native
screen downloads/imports verified Gandalf, configures background startup, and
performs an authenticated local model assessment before reporting **Ready**.
Startup at login and updates can be disabled during setup. Setup prepares the
default app, and SDKs register other apps automatically without permission prompts.
Credentials are written privately, never returning their contents to the browser.
Interrupted setup resumes without changing provider identity or
existing app credentials. No Python installation or terminal commands are needed.

The equivalent command is `e2emd --setup`, using the installed executable path
above on macOS/Windows. `--setup-headless` uses the same engine without a browser;
`--setup-status` checks the model and authenticated runtime connection. The
loopback UI uses a random session path, strict Host/Origin checks, bounded
requests and no external assets.

Package-managed setup references the installed executable rather than copying
it. Stop the runtime before upgrades and restart afterward so the new executable
is used. Package replacement preserves the private user grants and credentials.
User setup enables [idle background updates](UPDATING.md) by default. Verified
runtime payloads run from the private per-user update directory under a restart
supervisor; package-owned files and installer receipts remain intact. SDKs and
model updates use a separate signed descriptor. Clear the updates checkbox in setup
or pass `--no-auto-update` to `--setup-headless` to disable this behavior.
MSI uses a stable UpgradeCode and major-upgrade handling; Debian and RPM use the
stable `e2em-runtime` name; PKG uses `org.e2em.runtime`. Do not mix a previous
source-copy installation with package-managed setup: stop/uninstall the previous
user setup explicitly first.

Stop the runtime before package removal. Package removal retains user setup and
credentials, so reinstalling a package can reuse them. To erase enrolment, use
the legacy platform user setup tool's `uninstall` command before removing the package
(see `DESKTOP.md`). On macOS there is no generic PKG uninstaller: after stopping
the agent, remove only the payload files listed by
`pkgutil --files org.e2em.runtime`, then forget the receipt with
`sudo pkgutil --forget org.e2em.runtime`. Forgetting a receipt alone does not
remove payload files. Never recursively remove `/usr/local` or a user's home.

For Windows guided installations, stop the background process from Task Manager
and close E2EM Setup before running the legacy `uninstall` action. It removes the
managed current-user login entry, startup script, model store and enrolment. Then
remove the MSI using Add/Remove Programs. Package removal alone retains user data.

## Build locally

Use the native platform and an executable built for the selected target:

```sh
cargo build --locked --release --features runtime-service --bin e2emd --target x86_64-unknown-linux-musl
python3 scripts/package_runtime.py --format deb --target x86_64-unknown-linux-musl --binary target/x86_64-unknown-linux-musl/release/e2emd --output dist/runtime --inference-dir dist/inference
python3 scripts/package_runtime.py --format rpm --target x86_64-unknown-linux-musl --binary target/x86_64-unknown-linux-musl/release/e2emd --output dist/runtime --inference-dir dist/inference
```

Install the Rust target first. Linux tooling: `dpkg-deb` and `rpmbuild`.
The Linux daemon is musl-static. Its separate inference worker is built for glibc 2.28; the ONNX Runtime library needs glibc 2.27+ and libstdc++. GPU execution additionally needs a compatible NVIDIA driver. The native CUDA/cuDNN payload is larger than a CPU-only build and includes each component's licence notices. The initial Linux CI target is x86_64; arm64 staging is
supported but not an advertised tested build. macOS builds separate Intel and
Apple Silicon PKGs using `pkgbuild` and `productbuild`. Windows builds x64 MSI using WiX 4.0.6
(`dotnet tool install --global wix --version 4.0.6` and
`wix extension add -g WixToolset.UI.wixext/4.0.6`). Pass the corresponding
target and executable to `package_runtime.py`. Windows also needs the small native
GUI launcher: build with `--bin e2emd --bin e2em-setup` and pass
`--setup-binary target/x86_64-pc-windows-msvc/release/e2em-setup.exe`. Windows package CI uses
`RUSTFLAGS=-C target-feature=+crt-static` so the runtime does not require a
separate Visual C++ redistributable. `--stage-only` renders review
files without pretending native package creation occurred. `--version` supports
bounded three-part versions for installer upgrade tests.
WiX 4 targets .NET 6; CI selects .NET 8 with `DOTNET_ROLL_FORWARD=Major` and
checks the tool version before building. Use the same setting if your Windows
build host has only .NET 8 installed.

## CI and release boundaries

`.github/workflows/runtime-packages.yml` builds and lifecycle-tests native packages.
Native jobs also run the installed guided setup, authenticated model assessment, app
enrolment, credential preservation, login startup and reopen checks. Tagged releases
additionally run service conformance and SDK tests before publishing
installers, SDK archives, checksums and native reports through GitHub Releases.
The same native jobs stage standalone update payloads; release validation checks
their target executable format and includes all four host builds in `SHA256SUMS`.
See [release procedure](../RELEASING.md). Packages include the MIT license.

Checksums establish byte integrity, not publisher authenticity. Signing and
notarization remain future distribution milestones; installer artifacts are unsigned. Model manifests are separately Ed25519 signed.
Native test execution is reported by each release's workflow and attached reports.

Local Debian and Fedora install/upgrade/remove evidence is recorded in
`evidence/linux-packages-preview.json`. The musl release executable also passes
the 18 live service/SDK tests. These results do not qualify Windows or macOS.
References: [WiX Package](https://docs.firegiant.com/wix/schema/wxs/package/),
[Apple managed installs](https://developer.apple.com/library/archive/documentation/DeveloperTools/Conceptual/SoftwareDistribution4/Managed_Installs/Managed_Installs.html),
[RPM dependency generation](https://rpm.org/docs/6.0.x/manual/dependency_generators.html).

The release pipeline exports Gandalf once with PyTorch/ONNX parity checks, signs its manifest using `E2EM_MODEL_SIGNING_KEY`, stages matching native backends with `stage_inference.py`, then builds both installer variants. Offline lifecycle gates import the bundled model before completing setup. Native model qualification separately exercises every applicable default, custom selection, failure recovery and token coverage. See [model format and controls](MODELS.md).
