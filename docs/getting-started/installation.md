# Installation and environment

Rinzu supports JIT and AOT on Apple Silicon macOS (`aarch64-apple-darwin`) and x86_64 Linux with glibc (`x86_64-unknown-linux-gnu`). Building the compiler requires Rust 1.97.1. Native compilation uses Xcode Command Line Tools on macOS and Clang, an ELF linker, and the target C development files on Linux.

For a toolchain bundle matching your host, verify the archive, extract it to a new directory, and add its `bin/` to PATH. Run `rinzu doctor` from that installation to check the resource catalog. The distribution includes the official SDK sources for that target, so imports such as `import rinzu.codec;` resolve from that installation. See [Toolchain bundles](../tooling/distribution.md#export-the-toolchain).

## Prepare the toolchain

On macOS, install Apple's command-line development tools.

```sh
xcode-select --install
```

On x86_64 Linux, install Clang, an ELF linker, and the development files for glibc and GCC. Linux native SDK loading uses OpenSSL. On Debian 13, install `libssl-dev`, `libacl1-dev`, `libcurl4-openssl-dev`, `libsqlite3-dev`, and `libsdl2-dev` to build all available standard and official SDKs. Keep their corresponding runtime libraries available when launching the generated applications.

Use rustup to install Rust. The repository's `rust-toolchain.toml` selects the required toolchain and components.

```sh
rustup toolchain install 1.97.1 --profile minimal --component rustfmt --component clippy
git clone https://github.com/remi0753/rinzu.git
cd rinzu
cargo build --release -p rinzu-driver --bin rinzu
./target/release/rinzu --version
./target/release/rinzu doctor
```

Check the target and available execution modes printed by `doctor`. On x86_64 Linux, async JIT builds its accompanying ELF runtime with Clang. Build all workspace tools from source with `cargo build --locked --workspace`.

## Make rinzu available

Use `target/release/rinzu` directly, or add its directory to PATH for the current shell:

```sh
export PATH="$PWD/target/release:$PATH"
rinzu --version
```

The remaining pages use `rinzu` in commands. Toolchain version `0.5.0` supports source language version `0.5`. See the [migration notes](../../CHANGELOG.md#upgrade-from-040) when upgrading from toolchain `0.4.0`.

## Optional dependencies

| Use                 | Requirement                                                  |
| ------------------- | ------------------------------------------------------------ |
| macOS GUI and Metal | A macOS desktop session with the relevant GUI/GPU facilities |
| SDL2 SDK            | SDL2 and `sdl2-config`; Homebrew: `brew install sdl2`        |

Native builds on macOS target the running macOS version by default. Set `MACOSX_DEPLOYMENT_TARGET` to build with an explicit minimum such as `26.0`. The minimum must be macOS 11 or later and at least as recent as the minimum required by each linked library. Changing the target changes the native cache and lock identity.

Standard imports work without dependency paths. When using the executable from a source build, install its toolchain bundle for direct official SDK imports or declare explicit paths to the SDK sources. See [Manifests](../packages/manifest.md#dependencies).

Next: [Your first program](quickstart.md).
