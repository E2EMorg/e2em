# Contributing to E2EM

Thank you for helping improve the runtime and SDKs. Changes should focus on local runtime behavior, contracts, integration ergonomics, platform support, packaging, and documentation. Model training and research datasets belong in the wider project.

## Development setup

Use Rust 1.95+, Python 3.11+, and Node 22+. Dart SDK development uses Dart 3.12.1 (the package supports Dart 3.5+). C/C++ ABI tests need a C and C++ compiler. The rules preview needs no model, GPU, or Python inference worker.

```sh
cargo build --locked --workspace --all-features
cargo test --locked --workspace --all-features
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
python3 -m compileall -q scripts tests sdk/python
python3 -m unittest discover -s tests -p 'test_*.py'
node --test sdk/node/test.mjs
npx --yes --package typescript@5.9.3 tsc --strict --noEmit --module nodenext --moduleResolution nodenext --target ES2022 sdk/node/typecheck.ts
```

Linux C/C++ conformance after building:

```sh
E2EM_FFI_LIBRARY=target/debug/libe2em_ffi.so E2EM_ASAN=1 python3 -m unittest discover -s tests -p test_runtime_ffi.py -v
```

The Rust integration suite executes live Python/Node service fixtures. Ordinary Python discovery skips service/ABI checks when the required binary/library is absent; CI runs those explicitly.

Dart SDK checks run from `sdk/dart`: `dart pub get --enforce-lockfile`,
`dart analyze`, and `dart test`. After building the daemon, run
`python3 scripts/check_dart_sdk.py --binary target/debug/e2emd` from the root
for live shared-fixture conformance. CI runs Dart checks on all desktop targets.

## Contract changes

Rust `src/runtime/contract.rs` is the source of truth. Regenerate structural schemas and SDK types together:

```sh
cargo run --locked --example export_runtime_schema
python3 scripts/generate_runtime_bindings.py
```

Keep schemas, SDKs, conformance fixtures, and documentation consistent. Preserve original message text, coverage semantics, bounded work, fixed-code errors and revision guards. New behavior needs observable tests; bug fixes need regression coverage.

## Pull requests

Open an issue for a substantial contract or capability change. Use small, focused changes with an imperative subject. Explain the before/after behavior and verification. Include example inputs/outputs when an API changes. Do not commit credentials, local messages, model files, build outputs or data snapshots. Contributions are offered under this repository's MIT license.

## Repository layout

| Path | Purpose |
| --- | --- |
| `src/runtime/` | Contract, engine, scheduler, discovery, IPC and process boundaries |
| `src/rules.rs` | Preview email detector |
| `src/bin/e2emd.rs` | Native daemon |
| `crates/e2em-ffi/` | C ABI and C/C++ examples |
| `crates/e2em-platform/` | Narrow native OS adapters |
| `sdk/python/`, `sdk/node/`, `sdk/dart/` | Local service clients |
| `scripts/` | Installation, packaging, bindings and platform/resource checks |
| `tests/` | Runtime and SDK conformance, packaging and setup tests |
| `docs/` | User/developer guides and historical preview evidence |

## Releases

[Release procedure](docs/RELEASING.md) documents version checks, artifacts, checksums and native gates. GitHub CI and release workflows are the source of truth for executed checks. Historical evidence is retained under `docs/runtime/evidence`; it is not a substitute for a release's native results.
