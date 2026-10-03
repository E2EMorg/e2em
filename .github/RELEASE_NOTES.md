E2EM runtime and SDK developer preview. Personal email warnings, authenticated local IPC, and embedded Rust/C APIs. No model or GPU required.

**Start here:** [Installation guide](https://github.com/E2EMorg/e2em/blob/main/docs/INSTALL.md) · [SDK guide](https://github.com/E2EMorg/e2em/blob/main/docs/SDK.md) · [Changelog](https://github.com/E2EMorg/e2em/blob/main/CHANGELOG.md)

Choose an installer for your computer:

| Platform | Asset ending |
| --- | --- |
| Windows x64 | `x86_64-pc-windows-msvc.msi` |
| macOS Apple Silicon | `aarch64-apple-darwin.pkg` |
| macOS Intel | `x86_64-apple-darwin.pkg` |
| Debian / Ubuntu x86_64 | `x86_64-unknown-linux-musl.deb` |
| Fedora x86_64 | `x86_64-unknown-linux-musl.rpm` |

Installers are **unsigned**. Application enrolment and user-session startup remain explicit. Installing E2EM alone does not change other apps. This preview supports personal email-pattern warnings only; contextual models, block enforcement, browser transport, and sandbox brokers are not shipped.

Python wheel/source, Node `.tgz`, Rust source and platform C SDK archives are included. Registry publication is not configured. Linux C SDK libraries use the Ubuntu 24.04 runner baseline; Linux runtime installers are musl-static.

Native service checks and installer install/upgrade/remove gates passed before publication. Platform reports and `SHA256SUMS` are attached; checksums establish byte integrity, not publisher authenticity. Historical resource evidence is documented separately in the source tree.

Licensed under MIT. [Security boundaries](https://github.com/E2EMorg/e2em/blob/main/docs/runtime/SECURITY.md).
