# Install the E2EM runtime

[⬇ Download the runtime](https://github.com/E2EMorg/e2em/releases) · [README](../README.md) · [SDK guide](SDK.md)

Choose your platform under **Assets** on the newest release. Files start with `e2em-runtime-VERSION-` and end with the target and package type below. If GitHub collapses Assets, expand it. Python wheels and Node archives on the same page are for app developers, not the runtime installer.

| Computer | Runtime installer |
| --- | --- |
| Windows Intel / AMD 64-bit | `e2em-runtime-VERSION-x86_64-pc-windows-msvc.msi` |
| Mac M-series | `e2em-runtime-VERSION-aarch64-apple-darwin.pkg` |
| Mac Intel | `e2em-runtime-VERSION-x86_64-apple-darwin.pkg` |
| Ubuntu / Debian x86_64 | `e2em-runtime-VERSION-x86_64-unknown-linux-musl.deb` |
| Fedora x86_64 | `e2em-runtime-VERSION-x86_64-unknown-linux-musl.rpm` |

Installers are unsigned. macOS/Windows can warn about an unverified publisher; signed/notarized installers are a future release milestone. Only open an installer you obtained from this repository's release page. Check its SHA-256 against `SHA256SUMS` if needed. Checksums verify bytes, not publisher identity.

The same setup works on every platform: **install the package → open E2EM Setup → choose Set up E2EM**. The setup screen opens in your browser, downloads or imports verified Gandalf assets, starts the runtime in the background, and runs a local assessment before showing **Ready**. No terminal commands, Python installation, user account, or GPU are needed. Start at login and automatic updates are enabled by default, with checkboxes to opt out.

Offline installer filenames add `-offline-` after the version and include the same model. Select **Offline installation** in setup to disable all network activity. Interrupted setup can be retried with **Try again**; completed downloads and existing app credentials are preserved. Reopen E2EM Setup whenever you want to check readiness or connect another app. Installing E2EM does not change other applications until you connect them.

## Windows

1. Open the `.msi` file and follow the installer. It installs for your Windows user under `%LOCALAPPDATA%\E2EM Runtime`.
2. Leave **Open E2EM Setup** selected on the final installer screen, or open **Start → E2EM → E2EM Setup**.
3. Choose **Set up E2EM** and wait for **Ready**. E2EM runs in the background and starts when you sign in. You can close the setup tab with **Done**.

Python is only required if your application uses the Python SDK. No separate Visual C++ redistributable is required by the packaged daemon.

The app credential file is `%LOCALAPPDATA%\E2EM\app-my-app.json`. Do not paste its contents into issues or chats.

## macOS

Check **Apple menu → About This Mac**: a **Chip** such as M1/M2/M3 uses the Apple Silicon package; an Intel **Processor** uses the Intel package.

1. Open the matching `.pkg` and follow the installer. Package installation may request administrator permission.
2. Open **Applications → E2EM Setup**.
3. Choose **Set up E2EM** and wait for **Ready**, then select **Done**. No Terminal commands or Python installation are needed.

The LaunchAgent starts in your login session and at subsequent logins. The app credential file is `~/.config/e2em/apps/my-app.json`.

## Linux

These packages target Intel / AMD x86_64 Linux with glibc 2.28+ and libstdc++6. ARM Linux packages are not currently released. Guided setup needs a browser and a systemd user session.

Install the downloaded file from your Downloads directory, replacing the versioned filename with the one you downloaded.

Ubuntu / Debian:

```sh
sudo apt install ./e2em-runtime-0.1.2-x86_64-unknown-linux-musl.deb
```

Fedora:

```sh
sudo dnf install ./e2em-runtime-0.1.2-x86_64-unknown-linux-musl.rpm
```

Open **E2EM Setup** from your applications menu, choose **Set up E2EM**, and wait for **Ready**. Select **Done** to close setup; the runtime keeps running in the background. You can also open the same screen with `e2emd --setup` as your normal user.

The app credential file is `~/.config/e2em/apps/my-app.json`. The runtime process runs under your user. Python is only needed for Python integrations or the optional legacy setup scripts.

## Connect an application

On the **Ready** screen, enter the application name supplied by its integration and select **Connect app**. Use `my-app` for the [SDK examples](SDK.md), and a different name for each real app. Connecting an already enrolled app preserves its credentials. Credentials are written privately to your computer and are never sent to the setup page. The legacy scripts still support explicit credential rotation and revocation.

See [Python](../sdk/python/README.md), [Node / TypeScript](../sdk/node/README.md), or [embedded C/C++](../crates/e2em-ffi/README.md). Start with the [message-first SDK examples](SDK.md). Send a message, add optional context, and use all presets by default; policy files are optional. The native service accepts named categories in personal warning/review policies. The default model is Gandalf. Missing conversation and token truncation return incomplete coverage; supported custom models can be selected in the SDK. See [model provisioning and updates](runtime/MODELS.md). Platform authority remains unsupported.

## Upgrade or remove

User setup enables [idle background runtime updates](runtime/UPDATING.md).
Checks and downloads wait for 60 seconds without runtime work; completed
updates restart after outstanding requests and replies finish. Updates preserve
grants and credentials. Use `--update-status` or `--update-check-now` with the
grants path to inspect or request a check. Clear **Keep E2EM and its model up to date** during setup to disable updates, or select **Offline installation** to disable all network activity.

**Upgrade:** stop the runtime, install the new package, then reopen E2EM Setup to restart it. Private credentials and grants survive package replacement. Guided setup can safely reopen or resume an existing package-managed installation. Do not repeat the legacy `install` command over an existing installation. When upgrading from 0.1.1, setup provisions Gandalf and preserves app credentials.

**Remove:** stop the runtime first. Package removal preserves user grants by design. To erase enrolment too, run the legacy setup tool's `uninstall` action before removing the package. Linux users should disable the service and reload systemd; Mac users should unload the LaunchAgent. Windows users should stop the background runtime and remove its login entry before using Add/Remove Programs. See the package guide for exact commands.

## Unattended setup

Managed installations can use the same native setup engine without a browser. Run the installed executable as the target user:

```sh
e2emd --setup-headless
e2emd --setup-status
```

On macOS the executable is `/usr/local/libexec/e2em/e2emd`; on Windows it is `%LOCALAPPDATA%\E2EM Runtime\e2emd.exe`. `--setup-headless --offline` imports bundled assets without network access; `--no-auto-update` disables background updates. `--setup-status` reports a nonzero exit status until the installed model and authenticated runtime connection are ready. Use `--setup --setup-no-browser` to print the loopback setup URL for a manually opened browser on the same computer.

For exact removal commands, macOS PKG receipt handling, source installations, and lifecycle details, see [package ownership](runtime/PACKAGING.md#package-ownership-and-per-user-setup) and [desktop setup](runtime/DESKTOP.md).

## Troubleshooting

| Symptom | What to check |
| --- | --- |
| No runtime files in the release | Read the release notes; source archives and SDK files are separate from installers. |
| Setup was interrupted | Reopen E2EM Setup and select **Try again** or **Set up E2EM**. Completed downloads and app credentials are kept. |
| A different E2EM installation already exists | Stop and remove the previous source-copy installation before setting up the package-managed runtime. |
| `MODEL_UNAVAILABLE` | Check the runtime is running, the app is enrolled, and its credentials match this provider. Check `--model-status` with the grants path to verify that the selected model is installed. |
| Policy reference stops working after restart | Reconnect and validate the policy again; references belong to one provider instance. |
| App cannot access the endpoint from a sandbox | Shared access is not promised for sandboxed apps; use an embedded integration where permitted. |
| Unevaluated category | The category was accepted, but its detector or model is unavailable. Check `coverage.unevaluated_rules` and reason codes. |
| Unsupported rule or profile | Platform authority and block actions remain unsupported. Check policy structure and the provider capabilities. |

When reporting a problem, include your OS, release version, command, and fixed error code. Omit credentials and message contents.
