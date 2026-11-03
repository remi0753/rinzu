# C ABI and machine-readable schemas

Most applications use the language and SDK APIs. Native SDK authors and tooling authors may also need these lower-level resources.

## Native headers

The [include/rinzu](../../include/rinzu) directory defines runtime values, native records, callback entries/returns, typed transport, failure delivery, ownership transitions, managed roots, source async completion, and diagnostics.

Start with [value.h](../../include/rinzu/value.h), [native_record.h](../../include/rinzu/native_record.h), [callback_entry.h](../../include/rinzu/callback_entry.h), and [source_async.h](../../include/rinzu/source_async.h). Use these headers and generated descriptors together rather than copying internal layouts into an SDK.

Native source declarations are validated against supported scalar/record mappings. Classes and enums are not directly C structs. Managed record callback context tokens have call-bound validity. SDKs must contain foreign exceptions and preserve physical Owner and settlement contracts. See [Native interoperability](../language/native-interop.md).

## Schemas

| Schema | Purpose |
| --- | --- |
| [event-trace.schema.json](event-trace.schema.json) | Versioned runtime trace events |
| [resource-report-v1.schema.json](resource-report-v1.schema.json) | Resource accounting records |
| [package-api-v2.schema.json](package-api-v2.schema.json) | Public API snapshots |
| [package-lock-v2.schema.json](package-lock-v2.schema.json) | Checked package/dependency locks |
| [package-exports-v1.schema.json](package-exports-v1.schema.json) | Export descriptor wire format |
| [package-lock-v1.schema.json](package-lock-v1.schema.json) | Earlier lock wire format; new locks use v2 |

A JSON schema documents shape. The compiler additionally validates type/effect semantics, native capability scope, source identity, provider receipts, and compatibility. Passing shape validation alone does not authorize execution of native code.
