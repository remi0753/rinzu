# Lists and maps

Lists and maps are immutable snapshots. Operations that change their contents return a new collection, leaving aliases of the old value unchanged.

## Lists

```rinzu-test
fn sum(values: List<Int>) -> Int {
    var total = 0;
    var index = 0;
    let count = listLength(values);
    while index < count {
        switch listGet(values, index) {
            case Option.Some(value): {
                total = total + value;
            }
            case Option.None: {
                panic("invalid index");
            }
        }
        index = index + 1;
    }
    return total;
}

fn main() {
    let original: List<Int> = [10, 20];
    let extended = listAppend(original, 12);
    if sum(extended) != 42 || listLength(original) != 2 {
        panic("invalid list");
    }
}
```

| Operation | Result |
| --- | --- |
| `listLength(values)` | Int length |
| `listGet(values,index)` | `Option<T>` |
| `listAppend(values,value)` | New `List<T>` |
| `listSet(values,index,value)` | `Option<List<T>>` |
| `listRemove(values,index)` | `Option<List<T>>` |

Indices are Int. Invalid negative or out-of-range indices return None. Empty lists require an element type, for example `let names: List<String> = [];`.

## Maps

```rinzu-test
fn main() {
    let original: Map<String, Int> = {
        "a": 1, "b": 2
    };
    let changed = mapInsert(original, "a", 42);
    switch mapGet(changed, "a") {
        case Option.Some(value): {
            if value != 42 {
                panic("wrong value");
            }
        }
        case Option.None: {
            panic("missing key");
        }
    }
    if mapLength(original) != 2 {
        panic("original changed");
    }
}
```

Use `mapLength`, `mapGet`, `mapInsert`, `mapRemove`, and `mapKeys`. Keys require Key. `mapGet` returns Option; `mapRemove` returns a map even when the key was absent.

`mapKeys` follows first insertion order. Overwriting a key preserves its position; removing and reinserting it places it last. Duplicate literal keys use the later value without moving the first position.

## Costs and traversal

Use `while`, `listGet`, and `mapKeys` to traverse collections. List updates copy a flat representation, so repeated appends can accumulate quadratic work. Map operations can scan entries and compare key contents; do not assume constant-time lookup. Length and index APIs are not a promise of an optimized iterator representation.

A collection sent to a Worker must have a transferable key/value graph. Its snapshot is independent of Main's bindings.

Next: [Strings and bytes](strings-and-bytes.md).
