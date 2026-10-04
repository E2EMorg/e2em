# Background runtime updates

Managed user setup enables background updates on Linux, macOS and Windows.
The daemon checks the canonical `E2EMorg/e2em` GitHub release feed after **60
seconds without runtime work**, then at most once every **six hours** following
a successful check. The default channel accepts stable releases; including prereleases is an explicit opt-in. No app messages, credentials or policies
are sent to the update server.

All request operations and authentication count as work. Assessment work stays
active while queued, executing, cancelled but unfinished, or delivering its
reply. An open idle connection alone does not prevent updating. Network checks
and downloads run on a worker. If foreground work resumes, the download stops
at its next chunk boundary and retries after another idle period. Checks already
in progress have bounded network timeouts. A request never waits for an update
download to finish.

Once a verified payload is staged, restart waits for the idle window again.
Restart admission and request admission share a lock: after restart commits,
new connections must reconnect to the new daemon. Existing assessments and
their replies finish first. Apps should reconnect and revalidate policy
references after any runtime restart; this is the existing SDK lifecycle rule.

## Ownership and recovery

The original installed executable remains in place. A small launcher supervises
the selected daemon process. Updates are separate executable files under
`updates/` alongside `grants.json`, inside the private user configuration
directory. This works with package-owned binaries without administrative
permission, elevation dialogs, or changing installer receipts. SDK packages and
model assets are separate distributions and are not updated by this mechanism.

The updater selects a strictly newer semantic version, checks the host target,
requires the release's `SHA256SUMS`, bounds response/download sizes and verifies
the exact payload length and SHA-256. Downloads use HTTPS, including redirects.
Temporary payloads are atomically promoted only after verification. Cached
payloads are reverified before launch; symlinks/reparse points and shared Unix
update directories are rejected. A single supervisor lock prevents competing
launchers for the same configuration.

Before activation the launcher checks the executable's reported version. The
candidate must create its local service endpoint within 30 seconds and remain
running for a ten-second probation period. A failed probe, launch, startup,
probation, or later unexpected exit restores the previous working runtime,
falling back to the installed executable. An interrupted probation is rolled
back on the next start. The failed version is excluded until a newer release
appears. The child exits if its supervisor disappears. Normal termination
stops both processes and cleans the endpoint.

State writes are atomic; interrupted download files never become active. The
installed runtime remains available if updater metadata/storage prevents safe
launch; the updater logs the failure and retries initialization at the next
start. Only the active, previous and staged executable versions are retained. Grants and
app credentials remain intact. If a manually installed package catches up to
or passes the selected downloaded version, its executable takes precedence at
the next start.

Runtime payloads are unsigned. Checksums establish byte integrity; publisher
trust currently comes from HTTPS and the canonical GitHub repository. Release
signing and notarization remain distribution milestones.

## Controls

Direct daemon invocation keeps network updates off unless `--auto-update` is
present. The user setup tools add it to Linux/macOS startup configuration and
the Windows background startup configuration. Native guided setup offers an
updates checkbox; `--setup-headless --no-auto-update` applies the same opt-out. Existing startup configurations can add
the same flag after upgrading to an updater-capable executable.

| Daemon flag | Behavior |
| --- | --- |
| `--auto-update` | Enable the background updater and restart supervisor |
| `--no-auto-update` | Override enablement and run the original installed executable |
| `--update-channel preview` | Include published prereleases |
| `--update-channel stable` | Accept only stable releases (default) |
| `--update-idle-seconds 60` | Quiet window before update work/restart (1–86400) |
| `--update-check-seconds 21600` | Successful-check interval (300–604800) |
| `--update-status` | Print persisted JSON status; only `--grants` is required |
| `--update-check-now` | Request a check at the next idle opportunity |

For example on Linux:

```sh
e2emd --grants "$HOME/.config/e2em/grants.json" --update-status
e2emd --grants "$HOME/.config/e2em/grants.json" --update-check-now
```

macOS uses the installed `/usr/local/libexec/e2em/e2emd` with the same grants
path. On Windows:

```powershell
$Package = Join-Path $env:LOCALAPPDATA 'E2EM Runtime'
& "$Package/e2emd.exe" --grants "$env:LOCALAPPDATA/E2EM/grants.json" --update-status
& "$Package/e2emd.exe" --grants "$env:LOCALAPPDATA/E2EM/grants.json" --update-check-now
```

A check request also works before starting the daemon. It does not bypass idle
gating. Status includes the running version, channel, stage, available version,
last/next check times as Unix seconds, failure count and latest error. Failed
checks/downloads leave the service running and retry with exponential backoff
from five minutes up to the configured successful-check interval. Resuming
foreground work defers the update without counting as a failure.

To disable automatic updates during initial Unix setup, pass `--no-auto-update`
to its `install` command. Windows setup accepts `-NoAutoUpdate`. For an existing
installation, remove `--auto-update` from the startup command or add
`--no-auto-update`, then restart. The installed base executable is always
available for recovery. To reset the selected overlay, stop the runtime and
remove the managed update files before restarting; user `uninstall` removes
these files and preserves unrelated owner files.

## Release and test coverage

Release jobs stage `e2em-update-VERSION-TARGET.bin` (Unix) or `.exe` (Windows)
for Linux x86_64 musl, Windows x64, Intel macOS and Apple Silicon macOS. Release
validation requires all four payloads and includes them in `SHA256SUMS` before
publication. Payloads have no archive extraction paths or install scripts.

Offline tests cover idle admission, reply completion, resumed work, version
channels, bad checksums, truncated/oversized downloads, private storage, cached
corruption, retries, activation, rollback, interrupted trials, status controls,
supervisor loss, setup opt-out and removal. Native Windows qualification also
activates a local verified payload before running the shared service fixtures.
Unix lifecycle tests run on Linux and macOS CI. Live release-feed access is not
required by tests.
