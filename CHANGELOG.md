# Changelog

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
