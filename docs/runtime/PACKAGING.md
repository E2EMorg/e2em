# Runtime packages

The build produces `e2em-runtime-VERSION-TARGET.{msi,pkg,deb,rpm}` plus SHA-256
checksums and JSON metadata. Version comes from `Cargo.toml`. These are unsigned
developer-preview release artifacts. No models, grants,
credentials or diagnostic workers are included. Runtime inference needs no Python;
Unix user setup requires Python 3.11+. No package script starts a root service or
enrols applications automatically.

## Package ownership and per-user setup

Linux DEB/RPM installs `/usr/bin/e2emd` and setup/docs in `/usr/share/e2em`.
After package installation, run as your normal user:

```sh
python3 /usr/share/e2em/install_runtime.py install --binary /usr/bin/e2emd --use-packaged-binary
python3 /usr/share/e2em/install_runtime.py enrol my-app
systemctl --user daemon-reload
systemctl --user enable --now e2emd
```

macOS PKG installs `/usr/local/libexec/e2em/e2emd` and setup/docs in
`/usr/local/share/e2em`. In your GUI login session, with Python 3.11+ installed:

```sh
python3 /usr/local/share/e2em/install_macos_runtime.py install --binary /usr/local/libexec/e2em/e2emd --use-packaged-binary
python3 /usr/local/share/e2em/install_macos_runtime.py enrol my-app
launchctl bootstrap gui/$(id -u) "$HOME/Library/LaunchAgents/org.e2em.runtime.plist"
```

Windows MSI is per-user and installs under `%LOCALAPPDATA%\E2EM Runtime`.
It creates an Add/Remove Programs entry; it does not register a machine service.
Run user setup in PowerShell, then use the startup command it prints:

```powershell
$Package = Join-Path $env:LOCALAPPDATA 'E2EM Runtime'
& "$Package/install_windows_runtime.ps1" -Action install -Binary "$Package/e2emd.exe" -UsePackagedBinary
& "$Package/install_windows_runtime.ps1" -Action enrol -Principal my-app
```

Package-managed setup references the installed executable rather than copying
it. Stop the runtime before upgrades and restart afterward so the new executable
is used. Package replacement preserves the private user grants and credentials.
MSI uses a stable UpgradeCode and major-upgrade handling; Debian and RPM use the
stable `e2em-runtime` name; PKG uses `org.e2em.runtime`. Do not mix a previous
source-copy installation with package-managed setup: stop/uninstall the previous
user setup explicitly first.

Stop the runtime before package removal. Package removal retains user setup and
credentials, so reinstalling a package can reuse them. To erase enrolment, use
the platform user setup tool's `uninstall` command before removing the package
(see `DESKTOP.md`). On macOS there is no generic PKG uninstaller: after stopping
the agent, remove only the payload files listed by
`pkgutil --files org.e2em.runtime`, then forget the receipt with
`sudo pkgutil --forget org.e2em.runtime`. Forgetting a receipt alone does not
remove payload files. Never recursively remove `/usr/local` or a user's home.

## Build locally

Use the native platform and an executable built for the selected target:

```sh
cargo build --locked --release --features runtime-service --bin e2emd --target x86_64-unknown-linux-musl
python3 scripts/package_runtime.py --format deb --target x86_64-unknown-linux-musl --binary target/x86_64-unknown-linux-musl/release/e2emd --output dist/runtime
python3 scripts/package_runtime.py --format rpm --target x86_64-unknown-linux-musl --binary target/x86_64-unknown-linux-musl/release/e2emd --output dist/runtime
```

Install the Rust target first. Linux tooling: `dpkg-deb` and `rpmbuild`.
Linux packages use musl-static binaries so they do not accidentally inherit the
CI host's glibc baseline. The initial Linux CI target is x86_64; arm64 staging is
supported but not an advertised tested build. macOS builds separate Intel and
Apple Silicon PKGs using `pkgbuild`. Windows builds x64 MSI using WiX 4.0.6
(`dotnet tool install --global wix --version 4.0.6`). Pass the corresponding
target and executable to `package_runtime.py`. Windows package CI uses
`RUSTFLAGS=-C target-feature=+crt-static` so the runtime does not require a
separate Visual C++ redistributable. `--stage-only` renders review
files without pretending native package creation occurred. `--version` supports
bounded three-part versions for installer upgrade tests.
WiX 4 targets .NET 6; CI selects .NET 8 with `DOTNET_ROLL_FORWARD=Major` and
checks the tool version before building. Use the same setting if your Windows
build host has only .NET 8 installed.

## CI and release boundaries

`.github/workflows/runtime-packages.yml` builds and lifecycle-tests native packages.
Tagged releases additionally run service conformance and SDK tests before publishing
installers, SDK archives, checksums and native reports through GitHub Releases.
See [release procedure](../RELEASING.md). Packages include the MIT license.

Checksums establish byte integrity, not publisher authenticity. Signing and
notarization remain future distribution milestones; preview artifacts are unsigned.
Native test execution is reported by each release's workflow and attached reports.

Local Debian and Fedora install/upgrade/remove evidence is recorded in
`evidence/linux-packages-preview.json`. The musl release executable also passes
the 18 live service/SDK tests. These results do not qualify Windows or macOS.
References: [WiX Package](https://docs.firegiant.com/wix/schema/wxs/package/),
[Apple managed installs](https://developer.apple.com/library/archive/documentation/DeveloperTools/Conceptual/SoftwareDistribution4/Managed_Installs/Managed_Installs.html),
[RPM dependency generation](https://rpm.org/docs/6.0.x/manual/dependency_generators.html).
