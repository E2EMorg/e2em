# Local provider identity and isolation

> Platform qualification notes below preserve the source checkout's preview status.
> Current release workflow results and attached native reports show what has been
> executed for each release. Historical issue numbers refer to the source project.

The Linux preview admits owned mode-0700 endpoint directories and mode-0600 Unix
sockets. The provider verifies SO_PEERCRED UID (Tokio peer credentials), then an
explicit principal grant with a random 256-bit enrolment secret. Display app names
are never identities. The grant file is an owned private regular file, bounded
in size/count, and contains provider ID, principals, OS UIDs and secrets. Enrolment
and revocation are separate owner administration, never page/app commands.

Both parties use fresh nonces and HMAC-SHA256. The key is the UTF-8 representation
of the 64-character hexadecimal enrolment secret. The authenticated transcript is
NUL-separated `role, provider, principal, client_nonce, server_nonce`, including
a trailing NUL. Roles are `server` and `client`. The client checks the configured
provider ID and server proof before sending a client proof. The provider checks
caller UID/grant and client proof. Handshakes are size/time bounded. Python also
checks server peer UID on Linux; Node relies on private endpoint ownership and
mutual proof because Node provides no public peer-credential API.

Every operation reloads the grant metadata before execution. Removing/rotating a
grant prevents subsequent operations and new connections; the next operation on
an existing session disconnects and revokes its policies/work. Already completed
results cannot be revoked retroactively. Revocation is not an OS-level forced
process termination. Rotate the secret on re-enrolment; reconnect and revalidate
references. Never reuse a principal's old credential after revocation.

Policies are immutable and isolated by authenticated principal and provider boot
instance. Tokens bind policy contents. A caller cannot resolve another principal's
reference or cancel its request. Inline policies undergo the same capability and
personal-authority validation. Platform profiles have no grant in this preview
and are rejected. Per-principal queues are round-robin, with eight outstanding
assessments per principal, 64 globally and one scorer worker. Connections are
bounded to 32 and pending replies to eight per connection. Frames, output spans,
policy storage, handshakes and slow writes are bounded. Disconnect cancels that
connection's pending requests; startup refuses live endpoints and safely replaces
only owned, private stale sockets after connection refusal and inode revalidation.

## Trust boundaries and platform limits

A desktop UID authenticates an OS account, not an application executable. These
secrets provide logical principal separation between cooperating enrolled apps
and resist unauthorised protocol claims without the secret. A hostile same-user
process can often read credentials, inspect memory or replace owned endpoints.
This preview does not promise isolation against it. Root/administrators, compromised
OSes and hostile embedded hosts are outside the boundary. Strong app identity for
sandboxed/platform-owned enforcement requires a reviewed OS broker/entitlement
adapter, not caller-supplied names or profiles.

Linux tests run unconfined same-user applications. Flatpak, Snap, containers and
portal/broker service access are not advertised; embed the core where permitted.
macOS uses the portable Unix adapter and an explicit per-user LaunchAgent installer,
but native peer, launchd and sandbox/entitlement runs remain issue #24. Windows
uses a local-only pipe with a protected current-user/SYSTEM DACL, verifies the
connected process token's user SID, and requires the same explicit mutual grant
proof. Grant files must be regular non-reparse files owned by the current user;
every granting ACE must name that user or SYSTEM. The first-instance guard and
continuous server-handle ownership protect the namespace. SDKs reject remote or
non-E2EM pipe names before connecting. User SID authentication is still account
identity, not executable identity. Windows native ACLs, namespace protection,
installation and packaged-app access remain issue #23. Wine fails the native
first-instance exclusivity test, so its passing protocol fixtures do not qualify
that security property. Core/ABI cross-compilation is not service validation. Browser-origin and
extension enrolment require the separate milestone 2a bridge; no public localhost
HTTP listener or arbitrary native-command interface is exposed here.

## Privacy and resources

Assessment code opens only explicitly configured local paths/sockets and creates no
message files, message caches or telemetry. When enabled, the standalone daemon's
[background updater](UPDATING.md) fetches release metadata, checksums and runtime
payloads from the canonical GitHub repository over HTTPS during idle periods.
It sends no request text, findings, policies, grants or credentials. Embedded
hosts and daemon runs without `--auto-update` make no update connections. Runtime
errors contain fixed codes, not input text. Results do not echo matches. The
provider keeps policy metadata and ephemeral bounded requests/results; the app
owns conversation/draft retention. Service diagnostics deliberately omit request
content, findings and content-derived identifiers. The in-process Rust/C host is
responsible for its own logging/crash dump policy. Independent deployment security
and crash/retention audits remain pilot gates.

The loader drops its owned scorer and buffers after five minutes idle (configurable
for administration), timed from the last assessment completion. Idle connections
and capabilities calls do not reset the timer. Loading is coalesced by one worker;
pressure stops new admissions, finishes/cancels bounded in-flight work, then drops
resources. SIGUSR1 is a local administrator pressure hook, not an automatic OS
memory-pressure subscription. No resources are freed under scoring. Production
native workers that retain allocator/accelerator memory must use stronger disposal
or a disposable process; the preview's published RSS measures only rules. A
factory/scorer is trusted bounded code, and cancellation is best effort: arbitrary
blocking research scorers are not a production execution sandbox.

## Disposable backend boundary

A trusted provisioned native scorer may use `NativeProcessScorer` when dropping
an in-process backend leaves allocator caches resident. Versioned bounded stdio
frames and finite probabilities are mandatory. Startup and scoring timeouts,
protocol/worker failures and backend eviction stop, kill and reap its child;
failed workers are not reused. The child has no broker grant administration or
additional policy authority. No client-supplied executable is accepted, stderr is
suppressed, and response errors do not echo request text. This is a resource
lifetime boundary, not a sandbox for hostile native code. A trusted worker must
not leave descendants holding its pipes; backend-specific GPU/runtime cache
qualification is still required before advertising model inference.

Windows provisioning rejects broadened, inherited, foreign-owned and reparse
managed files before writing fresh secrets. Atomic file replacement can retain
a destination's DACL, so securing only the temporary file is insufficient.
Prepared native tests broaden the grant DACL deliberately, check refusal by both
the service and enrolment tool, and restore the private ACL before cleanup.
