# AOT builds and distribution

## Build an executable

```sh
rinzu build ./my-app --output ./dist/my-app
rinzu verify-aot ./my-app
./dist/my-app
```

The build command accepts an explicit `.rinzu` entry file and compiles without executing Main. AOT targets the operating system and architecture running `rinzu`. `verify-aot` compares execution with JIT on macOS arm64 and x86_64 Linux, including program output and exit status.

## Keep the output set together

An executable can have `.source-packages`, `.resources`, and native library sidecars. Move these with the executable and preserve their relative layout. Do not copy only the executable when a build generated companions.

On macOS, custom dylib load/re-export dependencies are bundled with relative loader paths. On Linux, explicitly selected custom ELF shared libraries travel with the source packages under their SONAME. Linux startup checks ELF build IDs, SHA-256 hashes, and symbol ownership. macOS startup uses Mach-O UUIDs for image identity. Source directories and SDK caches are unnecessary at runtime. Both platforms require compatible system libraries.

After changing toolchain, ABI, or source packages, rebuild generated artifacts and refresh intended package locks. Current source-package descriptors are version 2.6, and the checked SDK cache format is v4. API snapshots use v4 to describe private storage, generic method owners, interfaces and Error contracts; regenerate older snapshots before treating an API comparison as definitive.

## Build a macOS application

A language 0.5 application package can provide `macos-app.json`:

```json
{
  "schema": "rinzu.macos-app-v1",
  "identifier": "dev.example.myapp",
  "name": "My App",
  "version": "0.1.0",
  "build": "1",
  "minimumSystemVersion": "11.0",
  "resources": ["assets"]
}
```

```sh
rinzu bundle ./my-app --output './dist/My App.app'
rinzu bundle-verify './dist/My App.app'
```

The output must not already exist. Use `--config filename.json` for another package-local configuration. Bundling is package-only; standalone files and SDK packages cannot be application bundles.

`schema`, `identifier`, and `name` are required. Optional resources must be normalized package-relative files/directories without symlinks. Limits include 64 resource entries, 64 MiB per file, and 256 MiB total input. An optional icon is a 1024×1024 PNG or supported ICNS. The minimum system version is metadata, not a promise that every corresponding OS image is compatible.

The bundle contains its executable, frameworks, source-package runtime resources, application assets, receipts, and an ad-hoc signature. Bundle verification checks its generated inventory and signature. This does not perform Developer ID signing, notarization, or App Store submission. Follow the required platform distribution process separately when distributing outside development environments.

See [macOS applications](../libraries/macos.md) for SDK resource paths.

## Export the toolchain

```sh
rinzu toolchain-bundle --output ./rinzu-toolchain
```

On macOS arm64 or x86_64 Linux, choose a new directory whose parent exists. The bundle contains `bin/rinzu`, `bundle.json`, and the standard resources and official SDKs for the host under `share/rinzu`. Keep the directory together when relocating or installing read-only. Generated caches remain user-writable outside the installation.

The bundled executable resolves official SDKs such as `rinzu.codec` and `rinzu.sqlite` from source imports. It checks that the installation catalog matches its release and target, and verifies inputs for each SDK it uses. Use `rinzu doctor --sdk rinzu.sdl2` to check the installed catalog and SDL2 configuration. Native SDK builds use the local development toolchain. Both hosts bundle `rinzu.codec`, `rinzu.log`, `rinzu.sqlite`, and `rinzu.sdl2`. For macOS GUI applications, register the published `rinzu.macos` module. [macOS SDK setup](../libraries/macos.md)

For a tagged release, build from the tagged source with the lockfile and record the source commit in the bundle.

```sh
RINZU_SOURCE_COMMIT="$(git rev-parse HEAD)" cargo build --release --locked -p rinzu-driver --bin rinzu
release="$(./target/release/rinzu --version | cut -d ' ' -f 2)"
bundle="rinzu-${release}-aarch64-apple-darwin"
./target/release/rinzu toolchain-bundle --output "$bundle"
tar -czf "$bundle.tar.gz" "$bundle"
shasum -a 256 "$bundle.tar.gz" > "$bundle.tar.gz.sha256"
```

In the GitHub Release for that tag, summarize the main changes and migration steps, and link to the [changelog](../../CHANGELOG.md) at the tag. Attach the versioned archive and its `.sha256` file. Install by running `shasum -a 256 -c "$bundle.tar.gz.sha256"` alongside the archive, extracting to a new directory, and selecting the executable in its `bin/`. Keep each release in its own directory.
