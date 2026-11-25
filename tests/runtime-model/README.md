# Runtime model tests

This standalone Cargo package checks the current Rinzu native lifetime protocols with finite models and production event replay.

```sh
cargo xtask model
cargo xtask proofs
```

Shared deterministic vectors live in `tests/conformance`. The SMT checks require Z3 on `PATH` and also run under `cargo xtask test` and `cargo xtask ci`. Production compiler/runtime tests exercise the implementation and replay selected vectors and traces against the models.

The contracts are specified in [Future observation](../../docs/language/future-observation.md) and [native lifetime during cancellation](../../docs/abi/native-lifetime.md).

The [Future join proof](proofs/future-observation-join.smt2) checks the projection of the compiler's [pending map join](../../crates/compiler/src/error_effects/responsibility.rs) and [receiver coverage filter](../../crates/compiler/src/error_effects/responsibility/receivers.rs) for any producer token and receiver owner. An uncovered applicable alternative in either input survives a join. Applying that rule inductively covers any finite number of incoming alternatives. This result concerns the stored coverage for one receiver and assumes that the input states represent their alternatives.

The [native publication proof](proofs/native-publication-generation.smt2) checks the acquisition of DONE on the v2/v3 notification slot at a reused address. The model assumes the [NLS05 pin and publication rules](../../docs/abi/native-lifetime.md#nls05-publication-and-carrier-retirement). It uses the publication and load orders from the [C completion API](../../include/rinzu/native_completion.h) and [Rust completion implementation](../../crates/runtime/src/native_completion.rs). Under those premises, the acquisition comes from the active generation and orders its final SDK access before a v3 copy. The [production publication tests](../../crates/runtime/tests/published_payload_order.rs) exercise this ordering without SDK notification or helper join, and the [shared target tests](../../crates/runtime/tests/shared_target_lifetime_trace.rs) exercise storage reuse with a retained reference.

The [finite native lifetime model](src/native_lifetime.rs) separates logical decisions, SDK publication, receipt copying, helper return/join, call/callback environments, retained wake targets, and deferred drain ownership. Its tests exhaust bounded states, check unsafe mutations, and consume completion vectors also replayed by production C/Rust tests. State-limit errors mean incomplete verification. Separate component searches do not prove an unbounded implementation refinement or weak-memory correctness.

```sh
cargo test --manifest-path tests/runtime-model/Cargo.toml --test native_lifetime --test native_lifetime_vectors -- --nocapture
cargo test -p rinzu-runtime --test native_lifetime_vectors
```

The [completion/wake product](src/native_lifetime/composed.rs) adds shared-publication and all-receipt teardown checks. Production event traces are replayed with explicit internal steps; malformed and premature-release traces are rejected.

```sh
cargo test --manifest-path tests/runtime-model/Cargo.toml --test composed_lifetime -- --nocapture
cargo test -p rinzu-runtime --features test-native-lifetime-trace --test composed_lifetime_trace -- --nocapture
```

The [registration/publication extension](src/native_lifetime/registration.rs) starts from an unregistered carrier. It checks candidate-reference conservation, CAS/exchange ordering, failed-candidate rollback, and the same teardown conditions. Production replay requires the startup observations; it never silently substitutes the earlier post-registration prefix.

```sh
cargo test --manifest-path tests/runtime-model/Cargo.toml --release --config profile.release.debug-assertions=true --config profile.release.overflow-checks=true --test registration_lifetime -- --nocapture
```

## Historical prototypes

The original standalone ownership, cancellation, settlement, transport, callback and Host Loop experiments are available with `historical-prototypes`. The feature adds these test targets to the current model suite for historical reproduction.

```sh
cargo test --manifest-path tests/runtime-model/Cargo.toml --features historical-prototypes --test cancellation --test reload
```
