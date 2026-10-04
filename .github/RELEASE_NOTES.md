E2EM 0.1.2 now includes guided onboarding on every desktop platform. Install the package, open **E2EM Setup** from the application menu, and choose **Set up E2EM**. The setup screen installs and verifies Gandalf, starts the background runtime, enables startup at login, and confirms a real model assessment before showing **Ready**. Progress, offline operation, update preferences and application connection are available in the same screen. Windows also offers setup from the installer's finish screen. No Python installation or terminal commands are needed.

Gandalf is the default CPU model for the shared runtime, with all 40 default policies, optional conversation and custom policy strings. Normal setup downloads verified model assets; offline installers include the same weights. SDKs can select registered custom models and owner-authorized applications can install compatible model manifests from HTTPS URLs.

[Installation guide](https://github.com/E2EMorg/e2em/blob/v0.1.2/docs/INSTALL.md) · [SDK guide](https://github.com/E2EMorg/e2em/blob/v0.1.2/docs/SDK.md) · [Model format and controls](https://github.com/E2EMorg/e2em/blob/v0.1.2/docs/runtime/MODELS.md)

Choose the normal installer for your platform, or its `-offline-` counterpart to include Gandalf:

| Platform | Asset ending |
| --- | --- |
| Windows x64 | `x86_64-pc-windows-msvc.msi` |
| macOS Apple Silicon | `aarch64-apple-darwin.pkg` |
| macOS Intel | `x86_64-apple-darwin.pkg` |
| Debian / Ubuntu x86_64 | `x86_64-unknown-linux-musl.deb` |
| Fedora x86_64 | `x86_64-unknown-linux-musl.rpm` |

Installers are unsigned. Model descriptors are separately Ed25519 signed; assets are size/hash checked and candidates pass native smoke checks before activation. Model checks run every six hours when idle, retain the previous working version and support rollback. Offline setup disables network access. Application enrolment requires the user's approval in the setup screen.

Gandalf 0.0.1 is distributed under the owner-authorized MIT licence with upstream attribution, base-model licence and pinned provenance retained. The FP32 ONNX export passes comparison against the original checkpoint. Supporting all policies does not establish their accuracy: published policy thresholds are retained, other wordings use fallback thresholds. Missing required conversation and evidence exceeding 512 tokens produce an indeterminate review outcome. No per-category quality guarantee, block enforcement or sandbox broker is claimed.

Python wheel/source, Node archive, Rust source and native C SDKs accompany the runtime. Registry publication is not configured. Linux inference needs glibc 2.28+ and libstdc++; the daemon itself is musl-static. CPU inference needs no GPU or Python model packages.

Release publication requires native inference, installer install/upgrade/remove and guided onboarding gates on Linux, Windows and both Mac architectures. Reports and `SHA256SUMS` accompany the assets. Checksums establish byte integrity; model signatures do not sign the installers. This release's installers and SDKs have been rebuilt and replaced to include guided onboarding; `release-provenance.json` identifies the exact source commit and build run.
