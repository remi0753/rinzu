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

Rinzu supports JIT and AOT on **macOS arm64** and **x86_64 Linux with glibc**. Both targets support async tasks, Workers, and native SDKs. The source language version is `0.5` and the toolchain version is `0.3.0`.

[Documentation](docs/README.md) · [Introduction](docs/introduction.md) · [Installation](docs/getting-started/installation.md) · [Quick start](docs/getting-started/quickstart.md) · [Changelog](CHANGELOG.md)

## Build and run

On macOS arm64, install Rust 1.97.1 and Xcode Command Line Tools.

```sh
cargo build --release -p rinzu-driver --bin rinzu
./target/release/rinzu doctor
./target/release/rinzu run /path/to/hello.rinzu
```

On x86_64 Linux, install Rust 1.97.1, Clang, an ELF linker, and the glibc and GCC development files. Build the workspace tools and run an AOT executable.

```sh
cargo build --locked --workspace
./target/debug/rinzu run /path/to/hello.rinzu
./target/debug/rinzu verify-aot /path/to/hello.rinzu
./target/debug/rinzu build /path/to/hello.rinzu --output ./hello
./hello
```

See [Installation](docs/getting-started/installation.md) for Linux native SDK dependencies.

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
