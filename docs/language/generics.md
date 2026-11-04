# Generics and constraints

Functions, classes, enums, and type aliases can have type parameters. Type arguments can be explicit or inferred from arguments and an expected result type. All omitted parameters must have a unique solution; partial type-argument lists are unsupported.

```rinzu-test
class Box<T> {
    const value: T
}

fn identity<T>(value: T) -> T {
    return value;
}

fn equal<T: Equality>(left: T, right: T) -> Bool {
    return left == right;
}

fn main() {
    let boxed = Box(value: identity(42));
    if !equal(boxed.value, 42) {
        panic("generic failure");
    }
}
```

Types are invariant: `Box<A>` does not become `Box<B>` because a conversion or another relationship exists between A and B. Generic implementation code is specialized for concrete arguments.

## Local inference

Typed arguments and the expected result constrain parameters before unconstrained numeric literals use their ordinary local defaults. `same(7, CInt8(3))` can select CInt8; an already typed Int is never implicitly converted to match CInt8. Empty collections and `Option.None()` need a sufficient expected type. Later statements do not infer the type of an earlier local.

## Generic methods

A generic class or enum can define instance and static methods, and each method can declare additional type parameters. Write `receiver.map<String>(callback)` or `Box<Int>.create(value)`; omit arguments when local constraints determine them. Method parameters cannot shadow owner type parameters. Concrete unbound references such as `Box<Int>.map<String>` include the explicit self parameter. A receiver method reference does not implicitly capture its receiver.

## Marker constraints

Markers describe compiler-controlled capabilities. They are not user-implemented interfaces.

| Marker | Enables |
| --- | --- |
| `Equality` | Content equality through `==` and `!=` |
| `Key` | A value as a Map key |
| `Transferable` | Snapshot transfer into or out of a Worker |

Combine constraints with `+`, for example `fn compare<T: Transferable + Equality>(value: T) -> Bool`. Omitting a marker adds no declaration constraint, but operations still check their own requirements when the function is instantiated.

Integer, Boolean, text, byte, record, enum, and const-class capabilities depend on their concrete type graphs. Floats are not map keys. Lists and maps are not themselves map keys. Function values, mutable class graphs, and native handles cannot be transferred as ordinary immutable snapshots. Error and Future values cannot cross Worker boundaries.

## Interface constraints

Use `<T interface Reader>` for a user-defined interface constraint, `<T interface Reader + Named>` for several, and `<T: Transferable interface Reader + Named>` to combine sealed markers and interfaces. The colon introduces compiler-owned markers; the word `interface` introduces nominal behavior requirements. Type inference still selects concrete types from arguments and the expected result, then checks their declared conformance.

A constrained method call uses its interface's declared Error bound, including after specialization. A concrete receiver uses its ordinary method's Error bound. See [Interfaces](interfaces.md) for declarations, enum conformance and Self.

## Public generic APIs

An SDK can export generic functions, classes, and enums. Its source templates are instantiated by the consumer. Export types used by public signatures too. An unused template body is not proof that every possible instantiation will type-check.

Higher-kinded types, native generic declarations, and retaining an unspecialized polymorphic function value are unavailable. An expected concrete function type can specialize a generic function reference; `identity<Int>` explicitly names one specialization. Specialization has implementation limits, including a maximum of 256 instances and nesting depth 32.

Next: [Lists and maps](collections.md).
