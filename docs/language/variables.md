# Variables and constants

`let` creates a binding that cannot be reassigned. `var` creates a reassignable binding. Both require initializers; a type annotation is optional when the initializer determines the type.

```rinzu-test
fn main() {
    let unitPrice: Int = 100;
    var count = 2;
    count = count + 1;
    let total = unitPrice * count;
    if total != 300 {
        panic("unexpected total");
    }
}
```

Binding mutability and value mutability are separate. A class held in a `let` can still have mutable fields. An immutable list or record cannot be changed through a `var`; assign a new value instead.

Bindings belong to their block scope. Avoid names that shadow existing locals. Reassignment must preserve the declared or inferred type.

## Module constants

A top-level `let` defines a module constant with a pure initializer. Supported initializers include scalar calculations, Bool, String, and explicitly typed immutable aggregate values. Arbitrary function execution and global `var` declarations are unavailable.

```rinzu-test
let TITLE = "Report";

let MASK: CUInt8 = bitOr(0xf0, 0x0f);

let SIZES: List<Int> = [16, 32, 64];

fn main() {
    if TITLE != "Report" || MASK != CUInt8(255) || listLength(SIZES) != 3 {
        panic("invalid constants");
    }
}
```

Constant dependencies must not cycle. Use `export TITLE;` to expose a constant from its module. `const` on a class field controls reassignment of that field; module constants use `let`.

Next: [Types and conversions](types.md).
