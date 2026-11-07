# Installation and environment

Rinzu executes on Apple Silicon macOS (`aarch64-apple-darwin`). Building the compiler requires Rust 1.97.1. Native compilation uses Xcode Command Line Tools.

## Prepare the toolchain

Install Apple's command-line development tools:

```sh
xcode-select --install
```

Use rustup to install Rust. The repository's `rust-toolchain.toml` selects the required toolchain and components.

```sh
rustup toolchain install 1.97.1 --profile minimal --component rustfmt --component clippy
git clone https://github.com/remi0753/rinzu.git
cd rinzu
cargo build --release -p rinzu-driver --bin rinzu
./target/release/rinzu --version
./target/release/rinzu doctor
```

Check the target printed by `doctor`. Linux, Windows, and macOS x86_64 are portability compile targets; successful compiler builds there do not provide supported Rinzu execution.

## Make rinzu available

Use `target/release/rinzu` directly, or add its directory to PATH for the current shell:

```sh
export PATH="$PWD/target/release:$PATH"
rinzu --version
```

The remaining pages use `rinzu` in commands. Toolchain version `0.2.0` and source language version `0.5` describe different interfaces. For installation outside this checkout, see [Toolchain bundles](../tooling/distribution.md).

## Optional dependencies

| Use | Requirement |
| --- | --- |
| macOS GUI and Metal | A macOS desktop session with the relevant GUI/GPU facilities |
| SDL2 SDK | SDL2 and `sdl2-config`; Homebrew: `brew install sdl2` |
| Compiler C/ObjC formatting and analysis | LLVM 21; Homebrew: `brew install llvm@21` |
| Fuzz targets | Nightly Rust and cargo-fuzz |

Native builds target the running macOS version by default. Set `MACOSX_DEPLOYMENT_TARGET` to build with an explicit minimum such as `26.0`. The minimum must be macOS 11 or later and at least as recent as the minimum required by each linked library. Changing the target changes the native cache and lock identity.

Standard imports do not require dependency paths into this checkout. See [Manifests](../packages/manifest.md) for additional SDKs.

Next: [Your first program](quickstart.md).
