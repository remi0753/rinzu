# Compiler development

The Cargo workspace contains ABI definitions, tracing, the frontend, code generation, runtime, binding generation, driver, conformance commands, and xtask. Keep changes within the component that owns their behavior and update user documentation when observable syntax or APIs change.

## Verification baseline

The current source language and production toolchain define the baseline for verification. Write new specifications and ordinary fixtures for Rinzu `0.5`. When retaining compatibility, document the accepted older inputs, their behavior and supported paths, and the compatibility checks in the relevant specification. See the [supported compatibility contracts](docs/reference/support.md#compatibility).

## Build and check

```sh
cargo xtask fmt-write
cargo xtask fmt
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo xtask test
```

The production toolchain is selected by `rust-toolchain.toml`. The main executable is built with `cargo build -p rinzu-driver --bin rinzu`.

### x86_64 Linux builds

Cranelift lowering and object generation live under `crates/codegen/src/clif/`. The driver selects Clang and linker settings through `crates/driver/src/platform/`.

On an x86_64 Linux host with glibc, install [Clang, an ELF linker, and the development dependencies](docs/getting-started/installation.md#prepare-the-toolchain). Run `cargo build --locked --workspace` to build `rinzu`, `rinzu-conformance`, and `rinzu-xtask`. Run `target/debug/rinzu doctor`, build an application with `target/debug/rinzu build ./app --output ./app-elf`, and launch `./app-elf`. The Linux AOT toolchain uses `clang` on `PATH`. Set `RINZU_CLANG` to a specific executable or `RINZU_SYSROOT` to a separate Linux sysroot when needed.

On Apple Silicon, Cargo can build the Linux toolchain executable for `x86_64-unknown-linux-gnu` using the Rust target standard library and a Clang linker wrapper configured with an x86_64 Linux sysroot and ELF linker. Set `CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER` and `CC_x86_64_unknown_linux_gnu` to the wrapper and `AR_x86_64_unknown_linux_gnu` to `llvm-ar`. Use `cargo build --locked -p rinzu-driver --bin rinzu --target x86_64-unknown-linux-gnu`, then copy the resulting `rinzu` to a Lima VM configured with `vmType: qemu` and `arch: x86_64`. In that guest, run `rinzu build` and use `readelf -h` to check the ELF before running it. The workspace also has a Linux integration test at `cargo test -p rinzu-driver --test linux_aot`.

### Test suite

`cargo xtask test` runs the workspace with all targets and features, the Rust doctests, the current runtime models, the SMT checks, and the VS Code diagnostics unit tests on macOS arm64 and x86_64 Linux with glibc. Both platforms exercise the C callback and continuation fixtures and the Python Socket and HTTP loopback tests. macOS arm64 also runs the signed trampoline fixture and the driver tests for published `rinzu-test` examples. Install Python 3, Node.js, Z3 on `PATH`, and the native toolchain with address and undefined-behavior sanitizers. Set `RINZU_TEST_TOOL_TIMEOUT` in seconds to give compiler invocations and the Socket transfer fixture more time on an emulated host.

`cargo xtask differential` runs six current-source cases through `rinzu verify-aot` on both supported hosts. They check synchronous and async success, arithmetic failure, cancellation, overload, and shutdown. The command requires a nonempty successful test summary.

`cargo xtask proofs` runs the same Z3 checks included in `test` and `ci`. Missing Z3 and unexpected solver results fail the gate. The checks cover the abstract Future observation and native lifetime transitions.

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

C, C++, Objective-C, and headers use LLVM 21 clang-format/clang-tidy. `tools/native_quality.py` builds compilation contexts for the host target using Clang settings from the driver. The contexts cover generated runtime fragments and the target's SDK sources. `tools/clangd` starts the corresponding language server, and the `.vscode` settings select these tools. Native linting also needs SDK dependencies such as SDL2.

`tools/native_consumption.cpp` is compiled by the driver for supported closed-consumer analysis. Keep it as a compiler input. `tools/new_sdk.py` generates typed SDK scaffolds. Integration runners belong in `tests/integration`, not in the public language examples.

## Regression fixtures

`tests/fixtures/programs` holds programs consumed by compiler/codegen/driver tests. They are test inputs rather than a supported sample catalog. Other fixtures cover native ABI, callback, state, function, and HTTP parser regressions. Retain fixtures when a test references them, and update both paths and package-relative native/SDK dependencies when moving them.

```sh
cargo xtask sanitize
cargo xtask model
cargo xtask host-stress --seed 8241981735943431729 --rounds 100
cargo xtask differential
```

Sanitizer fixtures are under `tests/fixtures/c-abi`. Run the [current runtime models](tests/runtime-model/README.md) separately with `cargo xtask model`. Use `CARGO_TARGET_DIR` for an isolated Cargo build and `BUILD_DIR` for an isolated C sanitizer build.

The backend spike, resource profile, and v0.1 release sample are historical experiments available through their explicit commands or ignored tests. Run the original standalone C callback experiment with `make -C tests/fixtures/c-abi historical-sanitize`. The [standalone historical Rust models](tests/runtime-model/README.md#historical-prototypes) also require an opt-in. These runs provide evidence about the experiments' own inputs.

Run the VS Code integration tests with an installed VS Code from a desktop session. The runner creates a temporary workspace and profile. On macOS the process must be able to register a desktop application.

```sh
cargo build -p rinzu-driver --bin rinzu
python3 tests/integration/test_vscode_editor.py
```

The external macOS SDK repository runs Window, frame clock, sheet, and Metal checks. Use its `--ui` and `--require-gpu` flags for the desktop and GPU checks. The Rinzu suite exercises the native runtime and a relocatable signed `.app` using its own fixtures. [SDK validation commands](https://github.com/remi0753/rinzu-macos/blob/1aafc9967fdafcfdb3e44f2def3b2c6ee4f7640f/README.md#verify-the-sdk)

## Fuzzing and extended checks

The `fuzz/` package contains trace-schema and source-formatter fuzz targets. Install nightly Rust and cargo-fuzz for sanitizer instrumentation. `cargo xtask fuzz-build` builds both targets, and `cargo xtask fuzz 10` runs each for 10 seconds. Use `cargo xtask fuzz-replay <artifact> source_formatter` for formatter artifacts. The default replay target is `trace_schema`.

Run the opt-in JIT memory timing probe with `cargo test -p rinzu-codegen --lib readonly_allocation_cost_probe -- --ignored --nocapture`.

`cargo xtask ci --artifacts artifacts/ci` runs extended conformance checks and stores local reports. These checks have platform-specific SDK and fixture requirements. GitHub Actions is not configured; run the relevant build, formatting, and test commands locally for each change.

Build output, diagnostic captures, benchmark results, task notes, and progress records are not publication inputs. Keep them outside tracked source. The root ignore rules cover conventional local artifact directories.
