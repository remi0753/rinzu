# Runtime model tests

An executable model of runtime ownership, cancellation, settlement, transport, callback lifetimes, and host-loop behavior. It is a separate Cargo package, excluded from the production workspace.

```sh
cargo xtask model
```

Shared deterministic vectors live in tests/conformance. Model success is one check of protocol behavior; production compiler/runtime tests exercise the implementation itself.
