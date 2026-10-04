# Release procedure

Runtime and SDKs use one version and Git tag. API `0.1` and C ABI `1` have independent compatibility versions.

1. Update the versions in the root and all crate manifests, `sdk/python/pyproject.toml`, `sdk/node/package.json`, and `sdk/dart/pubspec.yaml`. Update examples/changelog for the new release.
2. Regenerate `Cargo.lock`, run contributor checks, and commit the changes.
3. Tag the commit `vMAJOR.MINOR.PATCH` and push the tag. For example: `git tag v0.1.2` then `git push origin v0.1.2`.
4. The release workflow checks tag/package version agreement, runs CI and native Windows/macOS service checks, builds and lifecycle-tests native installers, builds SDK distributions, and verifies packaged SDK imports.
5. Only after all jobs pass, it creates a stable release with installers, metadata, SDK packages, historical-independent native reports, and `SHA256SUMS`.

Artifacts include Windows x64 MSI; Intel/Apple Silicon macOS PKG; Linux x86_64 DEB/RPM; Python wheel and source distribution; Node `.tgz`; Dart SDK source archive; Rust workspace source archive; and native C ABI archives with libraries/header/license. Linux C libraries use the Ubuntu 24.04 runner baseline; the daemon is musl-static.

The four native daemon builds also ship as standalone background update payloads
(`e2em-update-VERSION-TARGET.bin`, or `.exe` on Windows). `package_update.py`
checks target executable formats. Publication requires all four payloads and
adds their hashes to `SHA256SUMS`. [Updater lifecycle and controls](runtime/UPDATING.md)
describe verification, idle gating, private per-user activation and rollback.

Failed release gates produce no public release. Diagnose the failing job and push a fix; use a new version/tag for an already-published release. Never overwrite published assets silently. The stable latest endpoint supplies the signed default model channel.

When the release owner explicitly requests replacement of the latest release,
dispatch **Release runtime and SDKs** from the reviewed source commit with
`release_tag` set to its existing version tag and `replace_existing=true`.
All native checks and normal/offline package gates still run. The workflow keeps
the previous assets, release metadata and tag in a `previous-release-backup`
artifact before publication. It replaces the assets, verifies the published
hashes, moves the existing tag to the built source commit and updates the release
notes with the build provenance. An interrupted publication restores the previous
assets and tag. `release-provenance.json` and `SHA256SUMS` identify the new build.

SDK registries are intentionally not configured. Publishing to PyPI/npm/crates.io requires separate owner setup and credentials. Windows signing/macOS notarization are not configured; releases explicitly identify installers as unsigned.

GitHub Actions needs `contents: write` only in the publication job. It uses the repository `GITHUB_TOKEN`; no personal release token or unrelated project credentials are needed.

Model builds require the repository secret `E2EM_MODEL_SIGNING_KEY` (32-byte Ed25519 key as hex), matching the committed public key. Never commit the private key. Approved Gandalf source/version are pinned in `build_gandalf_package.py`. Before changing them, retain upstream notices and the owner licence grant, rerun export parity, and pass native checks. The normal/offline installer pairs, standalone model assets, signed manifest, deployment ZIP and parity/native reports are all publication gates. Model version controls hot updates independently of the daemon version.
