# Manifests and dependencies

A package has a `rinzu.toml` manifest and a fixed `src/` source root. Use source language `0.5` for new code.

```toml
[package]
name = "my_app"
language = "0.5"
kind = "application"
entry = "src/main.rinzu"

[dependencies]
"example.values" = { path = "../values" }
```

`kind` defaults to `application`, and its entry defaults to `src/main.rinzu`. Package and module names use the supported lowercase identifier spelling. Entry paths must stay within `src/`, including after symlink resolution.

## Dependencies

A dependency key must match its package name. A `path` dependency refers to a directory relative to the declaring package. Built-in modules such as `rinzu.io` are reserved.

Import standard modules and official SDKs from the selected Rinzu toolchain. A toolchain bundle supplies `rinzu.codec`, `rinzu.log`, `rinzu.macos`, `rinzu.sqlite`, and `rinzu.sdl2` directly to source imports. Declare local and downloaded SDKs under `[dependencies]`. A dependency's private dependencies do not grant your package arbitrary import or native symbol access.

### Add a source archive

From your package directory, pass a public HTTPS URL or a local `.tar.gz` file to `modules add`.

```sh
rinzu modules add remi0753.test_sdk https://github.com/remi0753/testSDK/releases/download/v1.0.0/testSDK-1.0.0.tar.gz
rinzu modules sync
rinzu modules sync --offline
rinzu modules relock
```

The archive contains one pure Rinzu SDK. Put `rinzu.toml` and `src/` at its root or inside a single enclosing directory. The command records the `archive` dependency in `rinzu.toml`, the resolved graph in `rinzu.modules.lock`, and verified source under `modules/store/sha256/`. The lock and manifest are the inputs to commit. `modules sync` restores the pinned graph, and `--offline` restores it from installed sources, the user cache, or local archives. Build and editor commands use the installed sources locally.

The first addition from a GitHub Release checks the public asset digest. For an unpinned local file or other HTTPS URL, the first successful addition records the received SHA-256. Subsequent restorations enforce the pinned digest. A published SDK declares each of its direct external dependencies as an HTTPS archive with `integrity = "sha256:<digest>"`. When the current project is an SDK, `modules add` writes this integrity value for the dependency it adds. Consumers resolve the full graph from the published declarations and keep their own lock. A project's existing declaration is protected when adding a different source under the same name.

### SDK version requirements

A published SDK's `package.language` is the minimum language level its consumer must declare. The running compiler must also support the language levels used by the project and its SDKs. Use `[requires]` to declare a minimum Rinzu release when an SDK relies on APIs or functionality introduced in that release.

```toml
[package]
name = "example.values"
kind = "sdk"
language = "0.5"

[requires]
toolchain = "0.2.0"
```

`modules add` checks manifest requirements throughout the dependency graph before updating the project. `modules sync` restores the pinned sources. Compilation checks those requirements again and resolves the official API in the active toolchain. Published archives can use standard modules and archive-compatible official SDKs such as `rinzu.codec` and `rinzu.log`. Applications and local SDKs can also import official SDKs with additional native inputs.

An optional `[toolchain]` section with `version = "0.2.0"` in the root project selects an exact release. After changing the declared language level, run `rinzu modules relock` to refresh declaration fingerprints using the same installed and pinned sources.

## SDK manifests

```toml
[package]
name = "example.values"
language = "0.5"
kind = "sdk"
entry = "src/values.rinzu"

[exports]
modules = ["values", "extra"]
```

An SDK can contain pure source or native implementations. Its default entry is `src/<last-package-segment>.rinzu`; specifying entry is clearer. The entry's exported names are visible under the package name, while another public module appears as `package.module`. With no explicit exported modules, the entry is public.

## Native build inputs

An application or SDK can declare native inputs:

```toml
[native]
sources = ["native/src/api.c"]
headers = ["native/include/api.h"]
include-dirs = ["native/include"]
libraries = ["System"]
```

Other supported keys are `objects`, `search-paths`, `frameworks`, and `config-tools`. Paths are relative to their owning package. Headers support ABI checks; link inputs select providers. Unknown sections and keys are errors, so custom metadata should live in a separate file. `macos-app.json` holds application bundle settings.

## Locks and caches

```sh
rinzu lock ./my-app
rinzu lock ./my-app --check
```

`rinzu.lock` pins the checked dependency graph, source/native inputs, toolchain, and platform context. Existing locks are checked before execution/build and must be refreshed explicitly after an intended change. Use the same `-L`, `-l`, object, and header inputs when generating and checking a lock. `rinzu.modules.lock` pins the portable archive selection used to restore sources.

Caches live in the user's Rinzu cache directory; `RINZU_CACHE_DIR` selects a different root. Cached generated files and receipts are validated and can be rebuilt when invalid. A lock and a cache serve different purposes; neither is a remote package registry.

Next: [Authoring libraries and SDKs](authoring.md).
