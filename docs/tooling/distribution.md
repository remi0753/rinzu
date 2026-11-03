# AOT builds and distribution

## Build an executable

```sh
rinzu build ./my-app --output ./dist/my-app
rinzu verify-aot ./my-app
./dist/my-app
```

The same commands accept an explicit `.rinzu` entry file. Build compiles without executing Main. AOT uses the supported host target; it is not a cross-platform binary generator.

## Keep the output set together

An executable can have `.source-packages`, `.resources`, and native library sidecars. Move these with the executable and preserve their relative layout. Do not copy only the executable when a build generated companions.

Custom dylib load/re-export dependencies are bundled and use relative loader paths. Startup validates selected native images, descriptors, hashes, UUIDs, and ABI context. Source directories and SDK caches are not required at application runtime, but native OS image compatibility still applies. Building on one macOS version does not establish compatibility with arbitrary other OS builds.

After changing toolchain, ABI, or source packages, rebuild generated artifacts and refresh intended package locks. Current source-package descriptors are version 2.5, and the checked SDK cache format is v4. API snapshots use v3 to describe private storage and generic method owners; regenerate older snapshots before treating an API comparison as definitive.

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

Choose a new directory whose parent exists. The bundle contains `bin/rinzu` and `share/rinzu` resources. Keep both directories together when relocating or installing read-only. Standard package caches remain user-writable outside the installation. Additional SDK source is distributed separately.
