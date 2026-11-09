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
import rinzu.core;
import rinzu.number;

fn main() {
    try {
        let mask = rinzu.number.bitOr(CUInt8(0xf0), CUInt8(0x0f));
        let wrapped = rinzu.number.wrappingAdd(mask, CUInt8(1));
        let exact = rinzu.number.checkedAdd(CInt8(40), CInt8(2));
        if wrapped != CUInt8(0) || exact != CInt8(42) {
            rinzu.core.panic("unexpected arithmetic");
        }
    }
    catch problems {
        rinzu.core.panic("checked calculation failed");
    }
}
```

## Integer APIs

Import `rinzu.number` for these operations.

| API | Behavior |
| --- | --- |
| `rinzu.number.bitAnd(a,b)`, `rinzu.number.bitOr(a,b)`, `rinzu.number.bitXor(a,b)`, `rinzu.number.bitNot(a)` | Bit operations within the type's width |
| `rinzu.number.shiftLeft(value,count)`, `rinzu.number.shiftRight(value,count)` | `count: Int`. Negative or excessive counts throw Error. |
| `wrappingAdd/Sub/Mul(a,b)` | Arithmetic modulo the integer width |
| `checkedAdd/Sub/Mul(a,b)` | Return the value, or throw Error on overflow |

Binary numeric operands have the same type. Signed right shift is arithmetic; unsigned right shift is logical. Bit operations use named APIs instead of `&` or `|` operators.

## Floating point

Use `rinzu.number.abs(value)`, `rinzu.number.sqrt(value)`, and `rinzu.number.isNaN(value)`. `CFloat` uses 32-bit precision; `Float` and `CDouble` use 64-bit precision. The square root of a negative floating-point value is NaN. Text conversion is covered in [Strings and bytes](strings-and-bytes.md).

Next: [Control flow](control-flow.md).
