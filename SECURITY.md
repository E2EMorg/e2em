# Security policy

The 0.1 series is a developer preview. Read the [runtime trust boundaries](docs/runtime/SECURITY.md) before deployment; the current implementation does not promise isolation from hostile processes under the same user account or platform enforcement.

Report vulnerabilities privately through [GitHub private vulnerability reporting](https://github.com/E2EMorg/e2em/security/advisories/new). If private reporting is unavailable, open an issue asking maintainers for a private channel without including exploit details, credentials, or message contents. Ordinary bugs can use public issues.

Include the release version, platform, affected API and a minimal synthetic reproducer. Do not include enrolment secrets or real user messages. Maintainers will coordinate a fix and disclosure; no response-time guarantee is currently offered.
