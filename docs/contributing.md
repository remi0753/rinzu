# Compiler development

The Cargo workspace contains ABI definitions, tracing, the frontend, code generation, runtime, binding generation, driver, conformance commands, and xtask. Keep changes within the component that owns their behavior and update user documentation when observable syntax or APIs change.

## Build and check

```sh
cargo xtask fmt-write
cargo xtask fmt
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo xtask test
```

The production toolchain is selected by `rust-toolchain.toml`. The main executable is built with `cargo build -p rinzu-driver --bin rinzu`.

`cargo xtask test` runs the workspace with all targets and features, the Rust doctests, the standalone runtime model, the VS Code diagnostics unit tests, and the signed trampoline fixture on macOS arm64. The driver suite executes the published `rinzu-test` examples. The test command uses Python 3, Node.js, and the native toolchain. The Socket and HTTP SDK tests use local loopback listeners.

## Source and native quality tools

```sh
cargo xtask rinzu-fmt
cargo xtask rinzu-fmt-write
cargo xtask rinzu-lint
cargo xtask c-db
cargo xtask c-fmt
cargo xtask c-lint
```

The Rinzu checks use the actual parser, formatter, package contexts, and diagnostic fixtures. `fmt`, `fmt-write`, and `lint` combine Rinzu, Rust, and native gates. The formatting commands include the standalone runtime model and fuzz packages.

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

Sanitizer fixtures are under `tests/fixtures/c-abi`. The standalone runtime model is under `tests/runtime-model`. Run it separately with `cargo xtask model`. Use `CARGO_TARGET_DIR` for an isolated Cargo build and `BUILD_DIR` for an isolated C sanitizer build.

The [VS Code host tests](tooling/editor.md) need a desktop session. The macOS SDK runner at `tests/integration/test_macos_sdk.py` accepts `--ui` for Window and frame tests and `--require-gpu` to require Metal. Its default native checks report whether a Metal device was available.

## Fuzzing and extended checks

The `fuzz/` package contains trace-schema and source-formatter fuzz targets. Install nightly Rust and cargo-fuzz for sanitizer instrumentation. `cargo xtask fuzz-build` builds both targets, and `cargo xtask fuzz 10` runs each for 10 seconds. Use `cargo xtask fuzz-replay <artifact> source_formatter` for formatter artifacts. The default replay target is `trace_schema`.

Run the opt-in JIT memory timing probe with `cargo test -p rinzu-codegen --lib readonly_allocation_cost_probe -- --ignored --nocapture`.

`cargo xtask ci --artifacts artifacts/ci` runs extended conformance checks and stores local reports. These checks have platform-specific SDK and fixture requirements. GitHub Actions is not configured; run the relevant build, formatting, and test commands locally for each change.

Build output, diagnostic captures, benchmark results, task notes, and progress records are not publication inputs. Keep them outside tracked source. The root ignore rules cover conventional local artifact directories.
