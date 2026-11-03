# Generics and markers

Functions, classes, enums, and type aliases can have type parameters. Specify type arguments explicitly at use sites.

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
    let boxed = Box<Int>(value: identity<Int>(42));
    if !equal<Int>(boxed.value, 42) {
        panic("generic failure");
    }
}
```

Types are invariant: `Box<A>` does not become `Box<B>` because a conversion or another relationship exists between A and B. Generic implementation code is specialized for concrete arguments.

## Marker constraints

Markers describe compiler-controlled capabilities. They are not user-implemented interfaces.

| Marker | Enables |
| --- | --- |
| `Equality` | Content equality through `==` and `!=` |
| `Key` | A value as a Map key |
| `Transferable` | Snapshot transfer into or out of a Worker |

Combine constraints with `+`, for example `fn compare<T: Transferable + Equality>(value: T) -> Bool`. Omitting a marker adds no declaration constraint, but operations still check their own requirements when the function is instantiated.

Integer, Boolean, text, byte, record, enum, and const-class capabilities depend on their concrete type graphs. Floats are not map keys. Lists and maps are not themselves map keys. Function values, mutable class graphs, and native handles cannot be transferred as ordinary immutable snapshots. Error and Future values cannot cross Worker boundaries.

## Public generic APIs

An SDK can export generic functions, classes, and enums. Its source templates are instantiated by the consumer. Export types used by public signatures too. An unused template body is not proof that every possible instantiation will type-check.

Implicit type-argument inference, higher-kinded types, generic methods, native generic declarations, and retaining an unspecialized generic function value are unavailable. Specialization has implementation limits, including a maximum of 256 instances and nesting depth 32.

Next: [Lists and maps](collections.md).
