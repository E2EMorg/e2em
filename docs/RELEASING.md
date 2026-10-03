# Release procedure

Runtime and SDKs use one version and Git tag. API `0.1` and C ABI `1` have independent compatibility versions.

1. Update the versions in the root and both crate manifests, `sdk/python/pyproject.toml`, and `sdk/node/package.json`. Update examples/changelog for the new release.
2. Regenerate `Cargo.lock`, run contributor checks, and commit the changes.
3. Tag the commit `vMAJOR.MINOR.PATCH` and push the tag. For example: `git tag v0.1.1` then `git push origin v0.1.1`.
4. The release workflow checks tag/package version agreement, runs CI and native Windows/macOS service checks, builds and lifecycle-tests native installers, builds SDK distributions, and verifies packaged SDK imports.
5. Only after all jobs pass, it creates a **prerelease** with installers, metadata, SDK packages, historical-independent native reports, and `SHA256SUMS`.

Artifacts include Windows x64 MSI; Intel/Apple Silicon macOS PKG; Linux x86_64 DEB/RPM; Python wheel and source distribution; Node `.tgz`; Rust workspace source archive; and native C ABI archives with libraries/header/license. Linux C libraries use the Ubuntu 24.04 runner baseline; the daemon is musl-static.

Failed release gates produce no public release. Diagnose the failing job and push a fix; use a new version/tag for an already-published release. Never overwrite published assets silently. Download links point to the Releases listing because GitHub's `latest` endpoint excludes prereleases.

SDK registries are intentionally not configured. Publishing to PyPI/npm/crates.io requires separate owner setup and credentials. Windows signing/macOS notarization are not configured; releases explicitly identify installers as unsigned previews.

GitHub Actions needs `contents: write` only in the publication job. It uses the repository `GITHUB_TOKEN`; no personal release token or unrelated project credentials are needed.
