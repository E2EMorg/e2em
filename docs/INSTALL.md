# Install the E2EM runtime

[⬇ Download a developer preview](https://github.com/E2EMorg/e2em/releases) · [README](../README.md) · [SDK guide](SDK.md)

Choose your platform under **Assets** on the newest preview release. Files start with `e2em-runtime-VERSION-` and end with the target and package type below. If GitHub collapses Assets, expand it. Python wheels and Node archives on the same page are for app developers, not the runtime installer.

| Computer | Runtime installer |
| --- | --- |
| Windows Intel / AMD 64-bit | `e2em-runtime-VERSION-x86_64-pc-windows-msvc.msi` |
| Mac M-series | `e2em-runtime-VERSION-aarch64-apple-darwin.pkg` |
| Mac Intel | `e2em-runtime-VERSION-x86_64-apple-darwin.pkg` |
| Ubuntu / Debian x86_64 | `e2em-runtime-VERSION-x86_64-unknown-linux-musl.deb` |
| Fedora x86_64 | `e2em-runtime-VERSION-x86_64-unknown-linux-musl.rpm` |

The release is an unsigned developer preview. macOS/Windows can warn about an unverified publisher; signed/notarized installers are a future release milestone. Only open an installer you obtained from this repository's release page. Check its SHA-256 against `SHA256SUMS` if needed. Checksums verify bytes, not publisher identity.

Setup has three steps: **install the package**, **set up the runtime for your user**, and **enrol an app**. Installing the runtime alone does not change other applications. The preview supports apps that integrate email warnings. Application credentials stay private on your computer.

## Windows

1. Open the `.msi` file and follow the installer. It installs for your Windows user under `%LOCALAPPDATA%\E2EM Runtime`.
2. Open PowerShell as your normal user and run:

```powershell
$Package = Join-Path $env:LOCALAPPDATA 'E2EM Runtime'
& "$Package/install_windows_runtime.ps1" -Action install -Binary "$Package/e2emd.exe" -UsePackagedBinary
& "$Package/install_windows_runtime.ps1" -Action enrol -Principal my-app
```

3. Run the foreground startup command printed by the setup tool. Leave that PowerShell window open while using your app. Ctrl+C stops the runtime. This preview does not install a Windows background service or automatically start at login.

Python is only required if your application uses the Python SDK. No separate Visual C++ redistributable is required by the packaged daemon.

The app credential file is `%LOCALAPPDATA%\E2EM\app-my-app.json`. Do not paste its contents into issues or chats.

## macOS

Check **Apple menu → About This Mac**: a **Chip** such as M1/M2/M3 uses the Apple Silicon package; an Intel **Processor** uses the Intel package.

1. Open the matching `.pkg` and follow the installer. Package installation may request administrator permission.
2. Install Python 3.11+ if you do not already have it, then run these commands in Terminal as your normal user:

```sh
python3 /usr/local/share/e2em/install_macos_runtime.py install --binary /usr/local/libexec/e2em/e2emd --use-packaged-binary
python3 /usr/local/share/e2em/install_macos_runtime.py enrol my-app
launchctl bootstrap gui/$(id -u) "$HOME/Library/LaunchAgents/org.e2em.runtime.plist"
```

The LaunchAgent starts in your login session and at subsequent logins. The app credential file is `~/.config/e2em/apps/my-app.json`.

## Linux

These packages target Intel / AMD x86_64 Linux. ARM Linux packages are not currently released. You need Python 3.11+ for user setup and a systemd user session for the managed Linux service.

Install the downloaded file from your Downloads directory, replacing the versioned filename with the one you downloaded.

Ubuntu / Debian:

```sh
sudo apt install ./e2em-runtime-0.1.0-x86_64-unknown-linux-musl.deb
```

Fedora:

```sh
sudo dnf install ./e2em-runtime-0.1.0-x86_64-unknown-linux-musl.rpm
```

Then set up and start the runtime as your normal user:

```sh
python3 /usr/share/e2em/install_runtime.py install --binary /usr/bin/e2emd --use-packaged-binary
python3 /usr/share/e2em/install_runtime.py enrol my-app
systemctl --user daemon-reload
systemctl --user enable --now e2emd
systemctl --user status e2emd
```

The app credential file is `~/.config/e2em/apps/my-app.json`. The runtime process runs under your user, not root. Python is for setup and Python integrations; assessment runs in the native daemon.

## Connect an application

`my-app` is the application principal used by the [SDK examples](SDK.md). Enrol another slug for each app; re-enrolling rotates its secret. SDKs read the resulting credential file from your computer. They do not obtain credentials from web pages.

See [Python](../sdk/python/README.md), [Node / TypeScript](../sdk/node/README.md), or [embedded C/C++](../crates/e2em-ffi/README.md). The native service accepts personal email-warning policies only; unsupported policies fail validation.

## Upgrade or remove

**Upgrade:** stop the runtime, install the new package, then restart it. Private credentials and grants survive package replacement. Do not repeat the initial `install` user setup over an existing installation.

**Remove:** stop the runtime first. Package removal preserves user grants by design. To erase enrolment too, run the setup tool's `uninstall` action before removing the package. Linux users should disable the service and reload systemd; Mac users should unload the LaunchAgent. Windows users should stop the foreground process before uninstalling user setup and using Add/Remove Programs.

For exact removal commands, macOS PKG receipt handling, source installations, and lifecycle details, see [package ownership](runtime/PACKAGING.md#package-ownership-and-per-user-setup) and [desktop setup](runtime/DESKTOP.md).

## Troubleshooting

| Symptom | What to check |
| --- | --- |
| No runtime files in the release | Read the release notes; source archives and SDK files are separate from installers. |
| Setup says it is already installed | Follow the upgrade steps; user setup refuses to overwrite an existing installation. |
| `MODEL_UNAVAILABLE` | Check the runtime is running, the app is enrolled, and its credentials match this provider. The error code also covers unavailable rules-only providers. |
| Policy reference stops working after restart | Reconnect and validate the policy again; references belong to one provider instance. |
| App cannot access the endpoint from a sandbox | Shared access is not promised for sandboxed apps; use an embedded integration where permitted. |
| Unsupported rule or profile | Read `capabilities()`; preview 0.1 supports personal `pii.email` warnings/review only. |

When reporting a problem, include your OS, release version, command, and fixed error code. Omit credentials and message contents.
