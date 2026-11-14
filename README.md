# Rinzu

Rinzu is a statically typed, garbage-collected programming language with asynchronous tasks, explicit Worker execution, and native library integration. Use JIT execution during development and AOT compilation to distribute executables or macOS applications.

```rinzu
import rinzu.stdio;

fn main() {
    try {
        rinzu.stdio.writeLine("Hello, Rinzu!");
    }
    catch problems {
        panic("output failed");
    }
}
```

The execution target is **macOS arm64**. The source language version is `0.5`; the toolchain version is `0.3.0`.

[Documentation](docs/README.md) · [Introduction](docs/introduction.md) · [Installation](docs/getting-started/installation.md) · [Quick start](docs/getting-started/quickstart.md) · [Changelog](CHANGELOG.md)

## Build and run

Install Rust 1.97.1 and Xcode Command Line Tools, then run these commands in the repository:

```sh
cargo build --release -p rinzu-driver --bin rinzu
./target/release/rinzu doctor
./target/release/rinzu run /path/to/hello.rinzu
```

A standalone source file needs no manifest. Use a package with `rinzu.toml` for applications with additional SDK dependencies.

## Repository layout

| Directory | Contents |
| --- | --- |
| `crates/` | Compiler, code generation, runtime, CLI, and development commands |
| `sdk/` | Standard library sources and additional SDKs |
| [`examples/`](examples/inventory/README.md) | Runnable applications, including the HTTP inventory dashboard |
| `include/rinzu/` | Native boundary and runtime C headers |
| `docs/` | Language, library, and development documentation |
| `tests/` | Regression fixtures, integration runners, and runtime models |
| `fuzz/` | Source formatter and event trace fuzz targets |
| `tools/` | Native analysis, formatting support, SDK generation, and VS Code integration |

See [Compiler development](docs/contributing.md) to work on the implementation.
