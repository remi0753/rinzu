# Rinzu documentation

Learn to write Rinzu programs, use libraries, organize packages, test applications, and distribute them. These pages describe source language `0.5` and the toolchain in this repository. Commands use `rinzu`; substitute `target/release/rinzu` if it is not on your PATH.

Start with the introduction and follow the language chapters in order. A `rinzu-test` code block is a complete program that can be executed by the [documentation test runner](tooling/testing.md). Ordinary `rinzu` blocks can also contain file contents or fragments intended for a function body.

## Get started

1. [Introduction](introduction.md)
2. [Installation and environment](getting-started/installation.md)
3. [Your first program and project](getting-started/quickstart.md)

## Learn the language

1. [Source structure and syntax](language/syntax.md)
2. [Variables and constants](language/variables.md)
3. [Types and conversions](language/types.md)
4. [Operators and numeric operations](language/operators.md)
5. [Control flow](language/control-flow.md)
6. [Functions and closures](language/functions.md)
7. [Records and type aliases](language/records.md)
8. [Classes and methods](language/classes.md)
9. [Enums and Option](language/enums.md)
10. [Generics and constraints](language/generics.md)
11. [Interfaces](language/interfaces.md)
12. [Lists and maps](language/collections.md)
13. [Strings and bytes](language/strings-and-bytes.md)
14. [Errors and recovery](language/errors.md)
15. [Asynchronous tasks and Workers](language/async.md)
16. [Native interoperability](language/native-interop.md)

## Use libraries

- [Standard library overview](libraries/overview.md)
- [Core API reference](libraries/core.md)
- [SDK API references](reference/libraries.md)
- [Files, standard streams, and pull streams](libraries/io.md)
- [HTTP and TCP](libraries/networking.md)
- [JSON parsing and encoding](libraries/json.md)
- [Additional SDKs: SQLite, codecs, logging, and SDL](libraries/sdk.md)
- [macOS applications and GUI](libraries/macos.md)

## Organize and share code

- [Modules, imports, and exports](packages/modules.md)
- [Manifests and dependencies](packages/manifest.md)
- [Authoring libraries, SDKs, and API documentation](packages/authoring.md)

## Develop and distribute

- [CLI commands](tooling/cli.md)
- [Testing and documentation tests](tooling/testing.md)
- [Formatting, editors, and development restart](tooling/editor.md)
- [AOT builds and distribution](tooling/distribution.md)
- [Diagnostics and troubleshooting](tooling/diagnostics.md)

## Reference and implementation

- [Supported platforms and language boundaries](reference/support.md)
- [C ABI and machine-readable schemas](abi/README.md)
- [Third-party license inventory](reference/third-party-licenses.json)
- [Compiler development](contributing.md)
