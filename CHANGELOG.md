# Changelog

## 0.5.0

Rinzu 0.5.0 adds file-based tests, speeds up repeated JIT and AOT builds, and tightens the rules for observing asynchronous Errors. The source language remains `0.5`.

### Upgrade from 0.4.0

- Move source `test_*` functions into `tests/*.test.rinzu` files with their own `main`. Call `rinzu.testing.runCase` or `runAsyncCase` for named cases. Run `rinzu test ./app/tests` in place of `rinzu test ./app --unit`. File-based JSON reports use schema version 2. [Testing guide](docs/tooling/testing.md)
- Update exact `[toolchain]` pins to `0.5.0`, refresh native build locks with `rinzu lock`, and rebuild generated SDK and AOT artifacts. [Manifests and dependencies](docs/packages/manifest.md#sdk-version-requirements)
- Review diagnostics for fallible Futures that can be dropped, overwritten, or delegated to a receiver that can be cancelled before observing them. Await the result or transfer responsibility through a checked path. [Future observation](docs/language/future-observation.md)

### New and improved

- **Test files and named cases.** Discover and filter individual test files, run a named case with `--case`, and include explicit `rinzu-test` documentation examples in the same run. Each file has its own worker process and timeout. [Testing guide](docs/tooling/testing.md)
- **JIT startup and AOT builds.** Reuse checked inputs and compiled code across runs, reuse AOT runtime objects across compatible builds, and show JIT startup stages with `rinzu run --progress=always`. [JIT progress](docs/tooling/cli.md#jit-progress) · [Local compiler cache](docs/tooling/cli.md#local-compiler-cache)
- **Native async lifetimes.** Cancellation and timeout preserve the physical drain of work already started. The C ABI reference specifies when SDKs may reuse completion storage and release callback or notification resources. [Native lifetime](docs/abi/native-lifetime.md)
- **Bytes and editor support.** `rinzu.bytes.packInt64LE` encodes a list of integers as little-endian bytes. The local VS Code extension `0.3.0` improves syntax highlighting and retains diagnostics for multiple open packages. [Core APIs](docs/libraries/core.md) · [Editor setup](docs/tooling/editor.md#use-the-local-vs-code-extension)

## 0.4.0

Rinzu 0.4.0 adds x86_64 Linux with glibc and supports native SDK archives in the module manager. The source language remains `0.5`.

### Upgrade from 0.3.0

- Register the separately versioned `rinzu.macos` v0.1.0 with `rinzu modules add`. Keep `import rinzu.macos;` and commit the application manifest and modules lock. [macOS SDK migration](docs/libraries/macos.md)
- Update exact `[toolchain]` pins, refresh native build locks with `rinzu lock`, and rebuild generated SDK and AOT artifacts. [Manifests and dependencies](docs/packages/manifest.md)
- Install a toolchain bundle for your host. Both bundles include codec, log, SQLite, and SDL2. [Toolchain bundles](docs/tooling/distribution.md#export-the-toolchain)

### New and improved

- **Native SDK archives.** Register native source packages with `rinzu modules add`. The modules lock pins the archive and file inventory. Builds check the native inputs, including headers discovered through include paths, and `rinzu modules sync --offline` can restore cached packages. [Source archives](docs/packages/manifest.md#add-a-source-archive)
- **x86_64 Linux.** Run JIT and AOT with synchronous and asynchronous Main, Workers, and native SDKs. Keep the executable and ELF source-package sidecars together when distributing applications. [Installation](docs/getting-started/installation.md) · [Supported platforms](docs/reference/support.md)

## 0.3.0

Rinzu 0.3.0 adds an HTTP/1.1 server, locked SDK dependencies, and whole-file I/O on macOS arm64. The source language level remains `0.5`; review the updated Error contract syntax before upgrading.

### Upgrade from 0.2.0

- Move Error contracts immediately after the parameters. For example, change `fn read() -> String throws ReadFailed` to `fn read() throws(ReadFailed) -> String`, and `fn reset() -> Bool nothrow` to `fn reset() nothrow -> Bool`. This applies to declarations, callbacks, and interface requirements. [Error contracts](docs/language/errors.md#declare-an-error-upper-bound)
- Prefer the new `rinzu.error`, `rinzu.string`, `rinzu.bytes`, `rinzu.number`, `rinzu.future`, and `rinzu.native` modules. Legacy built-in spellings still resolve. When adopting `rinzu.error.code`, `context`, `withContext`, or `withCause`, handle their `Option` results. [Compatibility names](docs/libraries/core.md#compatibility-names)
- Install the matching toolchain bundle for direct imports of `rinzu.codec`, `rinzu.log`, `rinzu.macos`, `rinzu.sqlite`, and `rinzu.sdl2`. Explicit local SDK paths remain available for source checkouts. If your project uses `rinzu.lock`, review the changes and run `rinzu lock` to refresh it. Rebuild native SDK and AOT artifacts with the new toolchain. Projects that set an exact `[toolchain]` version must update it before building. [Manifests](docs/packages/manifest.md#sdk-version-requirements) · [Toolchain bundles](docs/tooling/distribution.md#export-the-toolchain)
- For the local VS Code extension (`0.2.0`), rebuild or select the updated Rinzu executable and reload the VS Code window. [Editor setup](docs/tooling/editor.md#use-the-local-vs-code-extension)

### New and improved

- **HTTP and TCP:** `rinzu.http.server` adds routing, sync and async handlers and middleware, in-memory requests for testing, and a plain HTTP/1.1 listener with keep-alive, ordered pipelining, deadlines, and graceful shutdown. The HTTP client adds method and header support, including reusable prepared headers. The codec can decode requests incrementally from borrowed socket buffers. The [inventory example](examples/inventory/README.md) shows the server and client working together. [Networking guide](docs/libraries/networking.md)
- **SDK dependencies:** Add pure Rinzu SDK archives from HTTPS URLs or local files. `rinzu.modules.lock` records the pinned dependency graph and hashes; `rinzu modules sync --offline` can restore it from sources already available locally. SDK authors can specify a minimum toolchain version with `[requires]`. [Manifests and dependencies](docs/packages/manifest.md)
- **File I/O:** `readBytes` and `readText` read a whole file with an explicit size limit; `replaceBytes` and `replaceText` atomically install a replacement. Text reads validate UTF-8. [File I/O guide](docs/libraries/io.md)
- **Language:** Function types can declare callback Error bounds, function type aliases can be exported, and dotted imports can use `as`. `rethrow` forwards all or selected caught Errors in their original order, preserving source information and traces. [Errors](docs/language/errors.md) · [Modules](docs/packages/modules.md#import-with-an-alias)
- **Diagnostics and editor:** CLI diagnostics share a renderer and show the source of Error contract failures and Future replacement paths. VS Code discovers the closest package, highlights Rinzu modules, and shows checked types and Error bounds on hover. Hover can answer without waiting for full diagnostics. [Diagnostics](docs/tooling/diagnostics.md) · [Editor guide](docs/tooling/editor.md)
