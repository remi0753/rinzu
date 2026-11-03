# Operators and numeric operations

| Operator | Operation |
| --- | --- |
| `+ - * / %` | Arithmetic and remainder |
| `-value`, `!condition` | Negation and Boolean inversion |
| `< <= > >=` | Ordering |
| `== !=` | Equality for types with Equality |
| `&&`, `||` | Short-circuit Boolean operations |
| `=` | Assignment to a binding or mutable field |

Use parentheses when precedence would be unclear. Ordinary integer overflow and division by zero are traps. Use checked arithmetic when a range failure should be recoverable.

```rinzu-test
fn main() {
    try {
        let mask = bitOr(CUInt8(0xf0), CUInt8(0x0f));
        let wrapped = wrappingAdd(mask, CUInt8(1));
        let exact = checkedAdd(CInt8(40), CInt8(2));
        if wrapped != CUInt8(0) || exact != CInt8(42) {
            panic("unexpected arithmetic");
        }
    }
    catch problems {
        panic("checked calculation failed");
    }
}
```

## Integer APIs

| API | Behavior |
| --- | --- |
| `bitAnd(a,b)`, `bitOr(a,b)`, `bitXor(a,b)`, `bitNot(a)` | Bit operations within the type's width |
| `shiftLeft(value,count)`, `shiftRight(value,count)` | `count: Int`; negative or excessive counts throw Error |
| `wrappingAdd/Sub/Mul(a,b)` | Arithmetic modulo the integer width |
| `checkedAdd/Sub/Mul(a,b)` | Return the value, or throw Error on overflow |

Binary numeric operands have the same type. Signed right shift is arithmetic; unsigned right shift is logical. Bit operations use named APIs instead of `&` or `|` operators.

## Floating point

Use `mathAbs(value)`, `mathSqrt(value)`, and `floatIsNan(value)`. `CFloat` uses 32-bit precision; `Float` and `CDouble` use 64-bit precision. The square root of a negative floating-point value is NaN. Text conversion is covered in [Strings and bytes](strings-and-bytes.md).

Next: [Control flow](control-flow.md).
