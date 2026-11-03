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

Only local path dependencies are supported. A dependency key must match its package name. Its path is relative to the declaring package. Built-in modules such as `rinzu.io` must not be listed as dependencies and cannot be replaced.

Declare every additional SDK directly used by your source. A dependency's private dependencies do not grant your package arbitrary import or native symbol access. There is no network registry, automatic fetch, version resolver, or package publish command.

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

A lock pins the checked dependency graph, source/native inputs, toolchain, and platform context. Existing locks are checked before execution/build and must be refreshed explicitly after an intended change. Use the same `-L`, `-l`, object, and header inputs when generating and checking a lock.

Caches live in the user's Rinzu cache directory; `RINZU_CACHE_DIR` selects a different root. Cached generated files and receipts are validated and can be rebuilt when invalid. A lock and a cache serve different purposes; neither is a remote package registry.

Next: [Authoring libraries and SDKs](authoring.md).
