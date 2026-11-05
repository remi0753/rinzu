# Records and type aliases

A language record is an immutable collection of named fields. Record types are structural: matching field names and types identify the same type regardless of declaration order or alias name.

```rinzu-test
type Point = { x: Int, y: Int };

type ReorderedPoint = { y: Int, x: Int };

fn moved(point: Point) -> Point {
    return { ...point, x = point.x + 1 };
}

fn main() {
    let original: Point = { x = 1, y = 2 };
    let equivalent: ReorderedPoint = { y = 2, x = 1 };
    let next = moved(original);
    if original != equivalent || next.x != 2 || original.x != 1 {
        panic("invalid record");
    }
}
```

## Construct a record

Use `field = expression` or add a type with `{ x: Int = 1 }`. An expected record type supplies the field types. An expression whose type is already determined can supply a field type too.

A bare numeric literal without an expected type is ambiguous inside a record. Write `let point: Point = { x = 1, y = 2 };` or explicit field annotations. Empty records, duplicate fields, missing fields, and extra fields are rejected.

Field evaluation follows source order. The type comparison ignores field order. A record with additional fields is not automatically a subtype of a smaller record.

## Copying and updating

Assignment copies the record value shallowly. Referenced immutable values can share storage. A class inside a record still refers to the same class instance in that Domain.

Fields cannot be reassigned. `{ ...base, field = value }` constructs a new structural record with the same fields and types. Evaluate base once, then update expressions in written order; publish the result only after all succeed. Updates are shallow, require at least one existing field, and cannot add fields, combine multiple bases, or update classes/native records. Overwriting a Future field does not remove the old Future’s observation responsibility.

## Type aliases

`type Pair<T> = { first: T, second: T };` gives a reusable spelling to a structural type. Aliases are transparent, can be generic, and cannot form cycles. They do not add methods, identity, or independent marker capabilities.

A language record and a `record` inside `native { ... }` have different representations and purposes. See [Native interoperability](native-interop.md).

Next: [Classes and methods](classes.md).
