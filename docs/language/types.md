# Types and conversions

Rinzu checks the types of bindings, parameters, and results before execution. Numeric width and signedness changes require explicit conversion. There is no `null`; use `Option<T>` for an ordinary missing value.

| Type | Meaning |
| --- | --- |
| `Unit` | Result of a function without a value |
| `Bool` | Boolean |
| `Int`, `Float` | Signed 64-bit integer and 64-bit floating point |
| `CInt8/16/32/64`, `CUInt8/16/32/64` | Integers with explicit widths and signedness |
| `CInt`, `CUInt` | 32-bit integers on the execution target |
| `CSize` | Unsigned 64-bit size on the execution target |
| `CFloat`, `CDouble`, `CBool` | Native scalar types |
| `String` | Immutable UTF-8 text |
| `Bytes(N)`, `ByteArray` | Fixed-size and variable-length bytes |
| `List<T>`, `Map<K,V>` | Immutable collections |
| `Option<T>`, `Future<T>`, `Error` | Absence, asynchronous completion, and ordinary failure |

Records, classes, enums, function types, and native opaque/record declarations add further types.

## Convert explicitly

```rinzu-test
fn main() {
    try {
        let small: CInt8 = CInt8(42);
        let wide: Int = Int(small);
        if wide != 42 {
            panic("wrong conversion");
        }
    }
    catch problems {
        panic("conversion failed");
    }
}
```

A conversion such as `CInt8(300)` throws an ordinary Error. It does not silently truncate or wrap. Use the [wrapping arithmetic APIs](operators.md) when wrapping is intentional.

Literals take types from annotations, parameters, results, and other expected-type contexts. Write an annotation for an ambiguous aggregate, for example `let values: List<CUInt8> = [1, 2];`.

## Type identity and capabilities

Record identity depends on field names and field types. Class and enum declarations create nominal types. A `type` alias introduces another spelling of a type without creating a new identity.

Equality, map keys, and Worker transfer have separate requirements: `Equality`, `Key`, and `Transferable`. The compiler checks the entire field graph. See [Generics and markers](generics.md).

Next: [Operators and numeric operations](operators.md).
