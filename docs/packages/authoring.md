# Authoring libraries, SDKs, and API documentation

A reusable library uses an SDK package, even when it contains no native code. Keep public declarations in its entry or explicitly exported modules.

## Create a source library

Create `values/rinzu.toml`:

```toml
[package]
name = "example.values"
language = "0.5"
kind = "sdk"
entry = "src/values.rinzu"
```

Create `values/src/values.rinzu`:

```rinzu
/// Return the supplied value without changing it.
fn identity<T>(value: T) -> T {
    return value;
}

export identity;
```

Check and build the package, then add it to an application's dependencies and call `example.values.identity<Int>(42)`.

```sh
rinzu check ./values
rinzu build ./values
```

Export public types used by signatures. Generic functions retain source templates and are checked when the consumer instantiates them. SDK build outputs include checked descriptors and native artifacts when required; they are not a binary-only package import format. Consumers still need the source package.

## Generate and compare API descriptions

```sh
rinzu api ./values --output ./values-api.json --markdown ./values-api.md
rinzu api ./values --output ./values-api-next.json --compare ./values-api.json
```

The JSON snapshot records exported types, signatures, constants, effects, and native boundary information. Comparison reports additions and conservative breaking candidates. Review semantics yourself: the tool does not automatically assign a semantic version or prove behavior unchanged.

Adjacent `///` comments attach to checked public declarations. Re-exports retain their original source identity. Comments can contain complete `rinzu-test` programs, which the generated Markdown keeps available to the documentation test runner.

```sh
rinzu test ./values --doc "$PWD/values-api.md" --format json
```

A documentation example must have a complete main and resolve imports through the selected package context. Plain `rinzu` fences are explanatory and are not executed.

## Generate a native scaffold

From the compiler checkout:

```sh
python3 tools/new_sdk.py ../my-native-sdk --name example.native_api --language c
```

Use `cpp` or `objcxx` for C++ or Objective-C++. The destination must not already exist. The scaffold contains a typed Rinzu wrapper, native result record, C header, implementation, manifest, and README.

Keep the exported C-compatible ABI stable. C++ and Objective-C++ exceptions must be contained inside the shim; do not unwind through managed frames. Specify allocator/release rules, physical Owner/thread requirements, callback lifetimes, cancellation behavior, and physical completion. A scaffold does not infer these contracts for an arbitrary library.

Use [Native interoperability](../language/native-interop.md), the [ABI headers](../abi/README.md), and the [compiler development tools](../contributing.md) when implementing the boundary. Distribute required source, headers, native inputs, and external library requirements together.
