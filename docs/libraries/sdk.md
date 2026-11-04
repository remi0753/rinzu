# Additional SDKs

Additional SDKs are ordinary source packages. Add a local dependency in `rinzu.toml`, then import their exported module. Paths are relative to your package, so adjust the example to your checkout layout.

```toml
[dependencies]
"rinzu.sqlite" = { path = "../rinzu/sdk/sqlite" }
"rinzu.codec" = { path = "../rinzu/sdk/codec" }
```

```rinzu
import rinzu.sqlite;
import rinzu.codec;
```

These dependencies are unavailable to manifest-free file input. The package system does not download packages from a registry; distribute or check out source dependencies explicitly.

## SQLite

[rinzu.sqlite](../../sdk/sqlite/README.md) provides Connection and Statement classes, scalar results, copied TEXT/BLOB results, bounded row snapshots, observers, deadlines, and cleanup APIs. A connection belongs to the physical Owner that acquired it. Finalize statements and await users before closing it. GC does not close a SQLite resource.

A synchronous query can block Main. For CPU-intensive queries, open, query, and close the connection entirely inside a synchronous Worker, catch ordinary failures there, and return a transferable success snapshot. Never transfer the connection itself.

`queryText` and `queryBytes` copy one row/column into managed storage with a 64 KiB result bound. Text decoding is strict UTF-8 and preserves embedded NUL. `queryByteRows` returns bounded owned snapshots. SQL, type, size, deadline, and ownership failures use Error. See the source for exact argument order and limits.

## Typed binary codecs

[rinzu.codec](../../sdk/codec/README.md) encodes and decodes Document, Settings, and Message values. These classes implement Encodable; call `rinzu.codec.encode(value)` or `value.encode()` to encode them through the shared contract. Each format is flat and versioned, with strict framing, UTF-8, length, and range checks. Document data is bounded to 1 MiB and messages to 64 KiB. These are specific typed formats, not a general JSON or recursive serialization framework.

Public operations are `encodeDocument/decodeDocument`, `encodeSettings/decodeSettings`, and `encodeMessage/decodeMessage`. Encoding produces ByteArray; decoding returns the corresponding const class or throws Error. Generate signature documentation with `rinzu api`.

## Logging

[rinzu.log](../../sdk/log/README.md) provides bounded asynchronous numeric-context logging. Await writes to apply backpressure. Rows are limited to 128 bytes. It does not implicitly dump payloads or raw Error values; choose the fields to report in application code.

## SDL2 and native UI

[rinzu.sdl2](../../sdk/sdl2/README.md) requires SDL2 and `sdl2-config`. Window and renderer operations use physical Main. It exposes copied events and a single active window per SDK image. Poll a bounded number of events per turn, await asynchronous application work, and explicitly destroy the window after its users settle.

For AppKit, Metal, documents, and native widgets, see [macOS applications](macos.md).

## Native SDK authoring helpers

[rinzu.shim_example](../../sdk/shim-example/README.md) demonstrates a typed native shim. Use `tools/new_sdk.py` to generate a C, C++, or Objective-C++ scaffold. [rinzu.consumption](../../sdk/consumption/README.md) demonstrates bounded synchronous callback consumption; its lifetime contract is narrower than general callback registration.

Continue with [Modules](../packages/modules.md) or [SDK authoring](../packages/authoring.md).
