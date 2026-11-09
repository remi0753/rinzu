# Compiler and runtime test inputs

`fixtures/programs` contains packages used by compiler, codegen, and driver regressions. Other fixture directories contain inputs for the native ABI, callbacks, functions, state, and HTTP parsing. The repository's Rinzu quality check also validates templates that need generated native declarations or a private copy of an SDK.

`integration` contains the Python runners used by Cargo, xtask, or a desktop test session. The executable C fixtures under `sdk/*/tests` stay with their SDK. `rinzu-runtime` runs the standalone native fixtures in `sdk_native`, and `rinzu-driver` exercises SDK programs in `sdk_programs` and the platform-specific suites.

`runtime-model` is a standalone Cargo package exercised by `cargo xtask test` and `cargo xtask model`. `conformance` contains deterministic vectors and trace-schema fixtures.

Generated binaries, test captures, and resource reports are local artifacts and are not committed.

[Compiler development](../docs/contributing.md)
