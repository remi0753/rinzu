# Lists and maps

Lists and maps are immutable snapshots. Updates return a new collection and preserve the contents of old aliases. Elements retain the same shallow references.

## Lists

```rinzu-test
import rinzu.list;

fn main() {
    let values = [1, 2, 3];
    let doubled = rinzu.list.map(values, fn (value: Int) -> Int {
        return value * 2;
    });
    let large = rinzu.list.filter(doubled, fn (value: Int) -> Bool {
        return value >= 4;
    });
    if large != [4, 6] || rinzu.list.length(values) != 3 {
        panic("invalid list");
    }
}
```

Use `for` to traverse a List. Use `rinzu.list.concatAll` to join a list of parts in order. Empty lists need a contextual element type, as in `let names: List<String> = [];`.

## Maps

```rinzu-test
import rinzu.map;

type Count = {key: String, value: Int};

fn main() {
    let counts: List<Count> = [{key = "red", value = 10}, {key = "blue", value = 20}];
    try {
        let index = rinzu.map.fromEntries(counts);
        var total = 0;
        for entry in rinzu.map.entries(index) {
            total = total + entry.value;
        }
        if total != 30 {
            panic("invalid count");
        }
    }
    catch [problem: rinzu.map.DuplicateKey<String>] {
        panic(problem.message);
    }
}
```

Use `rinzu.map.fromList` to index existing elements by a key derived from each element. Both bulk constructors report duplicates. Use `rinzu.map.insert` for replacement and addition. Map literals use the last value for duplicate keys and retain the first insertion position.

## Traversal and transfer

`rinzu.map.entries` materializes a List of key and value records in insertion order. The snapshot takes linear time and space. List transformations preserve order, and callbacks run synchronously. Existing Future observation rules apply when a callback fails or a transformation leaves elements out of its result.

A collection sent to a Worker must have a transferable key and value graph. Its snapshot is independent of Main's bindings.

See the [collection API reference](../libraries/collections.md) for signatures and Error contracts.

Next [Strings and bytes](strings-and-bytes.md).
