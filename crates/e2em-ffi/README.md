# Embedded C ABI 1

Prebuilt SDK archives on [GitHub Releases](https://github.com/E2EMorg/e2em/releases)
contain `include/e2em.h`, shared/static libraries under `lib/`, and C/C++ examples.
Make the shared library available to your platform loader, or link the static
library with the native dependencies reported by `cargo rustc --locked -p
e2em-ffi --lib -- --print native-static-libs`. Use the Rust SDK source archive
for a source build. The Linux C SDK uses the Ubuntu 24.04 runner baseline.

Build with `cargo build --locked --workspace`. Include `include/e2em.h` and link
`e2em_ffi` as a shared or static library. C and C++ examples are in `examples/`.
ABI 1 accepts API 0.1 values; `e2em_open(1)` returns a nonzero client handle.
Incompatible ABI versions return 0. Model installation is separate; this preview
has only the personal email warning capability.

Call `e2em_call(client, bytes, length)` with UTF-8 Call JSON, as defined by the
runtime schema. The input is copied/deserialized during the call; the caller
retains it. Capabilities, validation and cancellation return ready result handles.
Assessment returns a handle that may still be pending. Poll returns 0 pending,
1 ready or -1 invalid. Read only after ready: query the required byte count with
a null buffer, allocate that many bytes, then read again. Output is Reply JSON,
without a terminating NUL or call envelope. Insufficient capacity copies nothing.
No Rust allocation crosses the ABI.

Free every result handle with `e2em_result_free`, including pending handles
(which abandon/cancel their work). Close clients with `e2em_close`; late or
cancelled results authorize no operation. Disposal is idempotent. Handles are
never reused during normal process lifetime; concurrent calls/read/free/close
are serialized through checked registries, so a stale handle cannot dereference
freed storage. Already acquired calls may complete during concurrent close.
After completion, a result remains readable until explicitly freed.

There are at most 64 clients and 256 outstanding result handles per process.
Return 0 means invalid handle/input size, exhausted handle capacity or contained
panic; it must be treated as review/failure. Typed protocol failures are Error
replies. No panic crosses the ABI. C callers must provide valid input/output
memory for the declared lengths; arbitrary invalid pointers remain a caller bug.
Embedding makes the host the trust boundary. Numeric handles are lifetime tokens,
not an isolation boundary against hostile code in the same process.

Rust users can use `e2em_runtime::runtime::{Engine, scheduler::Scheduler}` directly.
`Scheduler::submit(...).await` and `Pending::wait()` share the same assessment
semantics, cancellation and revision requirements. Async completion is deliberately
poll/Future-based rather than callbacks into potentially disposed foreign objects.

Validation: Rust ownership/concurrency tests plus `tests/test_runtime_ffi.py` run
C/C++ through shared golden fixtures. Linux CI runs the latter with ASan/UBSan;
this checks the foreign caller buffers/lifetimes, while Rust registry tests cover
handle races. It is not a claim that all Rust machine code is sanitizer-instrumented.
