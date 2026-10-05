# Changelog

## 0.1.3

- Register SDK apps automatically without a Connect app approval step; provision default app settings during runtime setup.
- Treat complete warning results as advisory and continue without confirmation, retaining draft revision and coverage checks.
- Add the same native guided setup screen on Linux, macOS and Windows, with menu launchers, model provisioning, background startup, login/update preferences, app enrolment and an authenticated model assessment before reporting Ready. Python is no longer required for runtime onboarding.
- Resume interrupted setup and preserve existing app credentials; include guided onboarding checks in native package CI.
- Add idle background runtime updates, verified host payloads, release channels, status controls and bounded retries.
- Supervise per-user update activation with readiness probation, retained previous versions and automatic rollback; preserve package-owned files and grants.
- Enable updates in managed user setup, with opt-out, managed cleanup and offline lifecycle tests.

## 0.1.2

- Gandalf is the default native CPU model, provisioned during normal user setup or bundled in offline installers under MIT with upstream notices.
- Signed model manifests, resumable hash-verified downloads, idle model checks, atomic candidate activation and rollback.
- Custom model URLs/aliases across Rust, C, Python and Node; owner-controlled model installation.
- All 40 default policies, optional context and custom text; exact email detection plus explicit model fallbacks. Frozen Gandalf thresholds are retained.
- Token truncation reports incomplete coverage; bounded cancellation, disposable workers and recovery keep later requests usable. Assessment deadlines default to 15 seconds, with a 30-second maximum.

## 0.1.1 — First downloadable developer preview

- Fix Windows PowerShell enrolment replacement and enable explicit setup execution.
- Normalize native macOS static-link diagnostics and package path expectations.
- Preserve architecture-specific native reports and validate flattened release assets.


## 0.1.0 — Initial source preview (not distributed)

- Extract a dedicated runtime and SDK repository from the E2EM research project.
- Ship the local daemon, embedded Rust API and C ABI 1.
- Provide Python 3.11+ and Node 22+ clients with generated contract types.
- Add message-only Python/Node APIs and Rust helpers, 40 default presets, optional context, inline custom policies, and automatic loading of local app settings.
- Accept named and custom policies independently of evaluation ratings; report unavailable checks as unevaluated and preserve every score from supplied embedded model backends.
- Support personal `pii.email` warnings, revision guards, bounded scheduling, cancellation, and authenticated local IPC.
- Package Windows x64 MSI, Intel/Apple Silicon macOS PKG, and x86_64 Linux DEB/RPM installers.
- Attach SDK packages, native C SDKs, checksums and platform reports to releases.
- Publish installation, SDK, contract, security and contributor guides under MIT.

Installers are unsigned. No contextual model package, block enforcement, browser transport or sandbox broker is shipped.
