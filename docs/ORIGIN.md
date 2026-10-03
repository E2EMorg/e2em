# Source provenance

The runtime and SDKs were extracted from the original `semifmod` / E2EM research checkout at commit `14f6511` ("Fix per-user MSI identities and check installed package versions"). This is a fresh runtime-focused Git repository; research history, datasets, model artifacts, website sources, funding materials, and moderation/annotation frontends were not imported.

Imported components: `src/runtime`, the native daemon and resource probe, the C ABI/platform crates, Python/Node SDKs, runtime examples, schemas, conformance fixtures, runtime tests, installation/packaging scripts, and runtime documentation/evidence.

Extraction changes:

- Rename the Rust package/library to `e2em-runtime` / `e2em_runtime`.
- Reduce the bundled scorer to the existing email detector and its normalization; remove the research registry and lexical heuristics.
- Enable the daemon by default; keep diagnostic worker opt-in.
- Report the compiled package version in runtime capabilities.
- Apply the MIT license selected by the project owner to the new repository; retain dependency-specific licenses.
- Add download-first documentation, SDK package metadata, native release artifacts and publication checks.

Historical evidence in `runtime/evidence` comes from the source checkout and retains its original provenance and limitations. New CI runs validate this extraction separately. No research model qualification is implied.
