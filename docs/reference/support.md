# Supported platforms and language boundaries

## Versions and targets

| Interface | Current form |
| --- | --- |
| Source language | 0.5 |
| Toolchain crate / `rinzu --version` | 0.4.0 |
| JIT runtime | macOS arm64 and x86_64 Linux with glibc |
| AOT targets | macOS arm64 and x86_64 Linux with glibc, `x86_64-unknown-linux-gnu` |
| Build toolchain | Rust 1.97.1 |
| Native ABI | C-compatible ABI 1.1 with checked source-package descriptors |
| Source-package descriptor | 2.6 |
| SDK cache | v4 |
| Package lock (`rinzu.lock`) | v2 |
| Source archive lock (`rinzu.modules.lock`) | v1 |
| API snapshot | v4 |
| Toolchain bundle | v2 |
| Event trace / resource report | v1 |

Older language numbers in manifests remain accepted where supported by the loader. The frontend always uses the current grammar, so keep `language = "0.5"` and follow the [syntax migration notes](../../CHANGELOG.md#upgrade-from-020) when upgrading from toolchain `0.2.0`.

The [standard API migration](../libraries/core.md#compatibility-names) retains the legacy source spellings and reserves their ABI operation numbers. Compiled SDK descriptors and caches still follow the toolchain's compatibility fingerprints.

On x86_64 Linux, `rinzu run`, `rinzu test`, `rinzu build`, and `rinzu verify-aot` support synchronous and async Main, Workers, and native SDKs with SysV AMD64 callbacks. Linux async execution requires CET shadow stacks to be disabled for the process. Native `supplant` requires macOS arm64. Other Linux targets, Windows, and macOS x86_64 remain portability compile targets.

## Language boundaries

The language supports immutable structural records, nominal classes/enums, explicit generics, function expressions, interfaces, optional Error contracts, queued async tasks, Worker snapshots, and checked native declarations. The individual chapters describe their restrictions.

The following are outside the current public surface: inheritance, dynamic interfaces, operator overloading, nullable references, custom constructors/finalizers, arbitrary global initialization, range/for-await syntax and iteration over types other than List, shared mutable Worker memory, async Worker functions, and automatic live-code object migration.

Function values do not have Equality/Key/Transferable and can be stored in same-Domain Lists, Map values and Future results under the ordinary capture and observation checks. Native handles, Error, and Future cannot cross ordinary Worker transport. Native opaque values and native records cannot declare methods. Standard Result and legacy named-record/state/with/bind/taskScope syntax are unavailable.

## Library boundaries

| Area | Boundary |
| --- | --- |
| Package resolution | Local paths, locked source archives, and official SDKs from the toolchain |
| Files and streams | Bounded native operations, explicit physical Owner cleanup |
| HTTP client | Verified HTTPS or loopback HTTP; bounded methods, headers, and bodies; no automatic redirect/retry/proxy |
| HTTP server | Plain HTTP/1.1 over IPv4 TCP, bounded requests and connections; see the [listener limits](../../sdk/http/README.md#http11-listener) |
| TCP | Literal IPv4, Main ownership, bounded nonblocking operations |
| Native calls | Explicit provider scope and supported C mappings; arbitrary crashes are not recoverable Errors |
| Native callbacks | Selected call-lifetime paths; no general persistent capturing registration |
| GUI | AppKit and Metal via the external macOS arm64 SDK. [SDK setup](../libraries/macos.md) |
| Editor | Local VS Code adapter and versioned protocol; one folder/service, file documents |
| Distribution | Host AOT and generated bundles; no automatic notarization or App Store publishing |

These boundaries should inform API and deployment choices. Consult [SDK declarations](../libraries/overview.md) for exact limits, argument types, and ownership rules.
