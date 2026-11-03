# Compiler development

The Cargo workspace contains ABI definitions, tracing, the frontend, code generation, runtime, binding generation, driver, conformance commands, and xtask. Keep changes within the component that owns their behavior and update user documentation when observable syntax or APIs change.

## Build and check

```sh
cargo check --workspace --all-targets
cargo test --workspace --all-targets
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

The production toolchain is selected by `rust-toolchain.toml`. The main executable is built with `cargo build -p rinzu-driver --bin rinzu`.

## Source and native quality tools

```sh
cargo xtask rinzu-fmt
cargo xtask rinzu-fmt-write
cargo xtask rinzu-lint
cargo xtask c-db
cargo xtask c-fmt
cargo xtask c-lint
```

The Rinzu checks use the actual parser, formatter, package contexts, and diagnostic fixtures. `fmt`, `fmt-write`, and `lint` combine Rinzu, Rust, and native gates.

C, C++, Objective-C, and headers use LLVM 21 clang-format/clang-tidy. `tools/native_quality.py` builds compilation contexts for generated runtime fragments and SDK manifests; `tools/clangd` starts the corresponding language server. The `.vscode` settings select these tools. Native linting also needs SDK dependencies such as SDL2.

`tools/native_consumption.cpp` is compiled by the driver for supported closed-consumer analysis. Keep it as a compiler input. `tools/new_sdk.py` generates typed SDK scaffolds. Integration runners belong in `tests/integration`, not in the public language examples.

## Regression fixtures

`tests/fixtures/programs` holds programs consumed by compiler/codegen/driver tests. They are test inputs rather than a supported sample catalog. Other fixtures cover native ABI, callback, state, function, and HTTP parser regressions. Retain fixtures when a test references them, and update both paths and package-relative native/SDK dependencies when moving them.

```sh
cargo xtask sanitize
cargo xtask model
cargo xtask host-stress --seed 8241981735943431729 --rounds 100
cargo xtask differential
```

Sanitizer fixtures are under `tests/fixtures/c-abi`. The standalone runtime model is under `tests/runtime-model` and is excluded from the main workspace; `cargo xtask model` runs it explicitly. GUI tests need a real desktop session. Run platform-dependent checks in the environment their fixtures require.

## Fuzzing and CI

The `fuzz/` package contains trace-schema and source-formatter fuzz targets. Install nightly Rust and cargo-fuzz for sanitizer instrumentation. `cargo xtask fuzz-build` builds the configured trace target; use cargo-fuzz in `fuzz/` to select the formatter target.

`cargo xtask ci --artifacts artifacts/ci` runs the repository's combined checks and stores local reports. Build output, diagnostic captures, benchmark results, task notes, and progress records are not publication inputs. Keep them outside tracked source. The root ignore rules cover conventional local artifact directories.
