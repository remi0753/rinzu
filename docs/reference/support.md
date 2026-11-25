# Supported platforms and language boundaries

## Versions and targets

| Interface                                  | Current form                                                         |
| ------------------------------------------ | -------------------------------------------------------------------- |
| Source language                            | 0.5                                                                  |
| Toolchain crate / `rinzu --version`        | 0.5.0                                                                |
| JIT runtime                                | macOS arm64 and x86_64 Linux with glibc                              |
| AOT targets                                | macOS arm64 and x86_64 Linux with glibc, `x86_64-unknown-linux-gnu`  |
| Build toolchain                            | Rust 1.97.1                                                          |
| Native ABI                                 | C-compatible ABI 1.1 with checked source-package descriptors         |
| Source-package descriptor                  | 2.6                                                                  |
| SDK cache                                  | v4                                                                   |
| Package lock (`rinzu.lock`)                | v2                                                                   |
| Source archive lock (`rinzu.modules.lock`) | v1                                                                   |
| API snapshot                               | v4                                                                   |
| Toolchain bundle                           | v2                                                                   |
| Event trace / resource report              | v1                                                                   |
| Test report (`rinzu.test-report`)          | v2 for file tests, including mixed runs. v1 for document-only tests. |

On x86_64 Linux, `rinzu run`, `rinzu test`, `rinzu build`, and `rinzu verify-aot` support synchronous and async Main, Workers, and native SDKs with SysV AMD64 callbacks. Linux async execution requires CET shadow stacks to be disabled for the process. Native `supplant` requires macOS arm64. Other Linux targets, Windows, and macOS x86_64 remain portability compile targets.

## Compatibility

A standalone file uses Rinzu `0.5` source semantics. Package manifests accept `0.1` through `0.5` for compatibility, with the current frontend's grammar and semantic checks. The version participates in [source archive requirements](../packages/manifest.md#sdk-version-requirements). Declare `language = "0.5"` for current packages and follow the [syntax migration notes](../../CHANGELOG.md#upgrade-from-020) when upgrading older source.

A `0.1` manifest is an application manifest with only the `[package]` section and its `name`, `language`, and optional `entry` keys. Its AOT build and ordinary JIT/AOT comparison use the legacy baseline path. Current async AOT and async comparison use the current package contract.

The [standard API compatibility names](../libraries/core.md#compatibility-names) retain earlier spellings and reserve their ABI operation numbers. [Native completion layouts v1 through v3](../abi/native-lifetime.md#nls05-publication-and-carrier-retirement) are part of the current ABI contract. Compiled SDK descriptors and caches follow the toolchain's compatibility fingerprints.

## Language boundaries

The language supports immutable structural records, nominal classes/enums, explicit generics, function expressions, interfaces, optional Error contracts, queued async tasks, Worker snapshots, and checked native declarations. The individual chapters describe their restrictions.

The following are outside the current public surface: inheritance, dynamic interfaces, operator overloading, nullable references, custom constructors/finalizers, arbitrary global initialization, range/for-await syntax and iteration over types other than List, shared mutable Worker memory, async Worker functions, and automatic live-code object migration.

Function values do not have Equality/Key/Transferable and can be stored in same-Domain Lists, Map values and Future results under the ordinary capture and observation checks. Native handles, Error, and Future cannot cross ordinary Worker transport. Native opaque values and native records cannot declare methods. Standard Result and legacy named-record/state/with/bind/taskScope syntax are unavailable.

## Library boundaries

| Area               | Boundary                                                                                                                            |
| ------------------ | ----------------------------------------------------------------------------------------------------------------------------------- |
| Package resolution | Local paths, locked source archives, and official SDKs from the toolchain                                                           |
| Files and streams  | Bounded native operations, explicit physical Owner cleanup                                                                          |
| HTTP client        | Verified HTTPS or loopback HTTP; bounded methods, headers, and bodies; no automatic redirect/retry/proxy                            |
| HTTP server        | Plain HTTP/1.1 over IPv4 TCP, bounded requests and connections; see the [listener limits](../../sdk/http/README.md#http11-listener) |
| TCP                | Literal IPv4, Main ownership, bounded nonblocking operations                                                                        |
| Native calls       | Explicit provider scope and supported C mappings; arbitrary crashes are not recoverable Errors                                      |
| Native callbacks   | Selected call-lifetime paths; no general persistent capturing registration                                                          |
| GUI                | AppKit and Metal via the external macOS arm64 SDK. [SDK setup](../libraries/macos.md)                                               |
| Editor             | Local VS Code adapter and versioned protocol; one folder/service, file documents                                                    |
| Distribution       | Host AOT and generated bundles; no automatic notarization or App Store publishing                                                   |

These boundaries should inform API and deployment choices. Consult [SDK declarations](../libraries/overview.md) for exact limits, argument types, and ownership rules.
