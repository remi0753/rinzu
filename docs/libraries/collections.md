# Collection API reference

Import `rinzu.List` or `rinzu.Map` and qualify calls with the module name. Collection types are spelled `List<T>` and `Map<K,V>`. Map keys satisfy `Key`.

## rinzu.List

| Function | Result |
| --- | --- |
| `rinzu.List.length(List<T>)` | `Int` |
| `rinzu.List.get(List<T>, Int)` | `Option<T>` |
| `rinzu.List.append(List<T>, T)` | `List<T>` |
| `rinzu.List.set(List<T>, Int, T)` | `Option<List<T>>` |
| `rinzu.List.remove(List<T>, Int)` | `Option<List<T>>` |
| `rinzu.List.map(List<T>, fn (T) -> U)` | `List<U>` in input order |
| `rinzu.List.filter(List<T>, fn (T) -> Bool)` | Matching elements in input order |
| `rinzu.List.concatAll(List<List<T>>)` | `List<T>` in part and element order |

Negative and out-of-range indices return `Option.None`. Updates preserve existing snapshots. `map` and `filter` call their functions synchronously, once per visited element, from the start of the input. A callback Error propagates immediately with its original failure information.

## rinzu.Map

| Function | Result |
| --- | --- |
| `rinzu.Map.length(Map<K,V>)` | `Int` |
| `rinzu.Map.get(Map<K,V>, K)` | `Option<V>` |
| `rinzu.Map.insert(Map<K,V>, K, V)` | `Map<K,V>` |
| `rinzu.Map.remove(Map<K,V>, K)` | `Map<K,V>` |
| `rinzu.Map.keys(Map<K,V>)` | `List<K>` in insertion order |
| `rinzu.Map.entries(Map<K,V>)` | `List<{key: K, value: V}>` in insertion order |
| `rinzu.Map.fromList(List<T>, fn (T) -> K)` | `Map<K,T>` in input order |
| `rinzu.Map.fromEntries(List<{key: K, value: V}>)` | `Map<K,V>` in input order |

A missing key returns `Option.None` from `get` and leaves the contents unchanged after `remove`. `insert` replaces a matching value at its original position and appends a new key at the end. Removing and reinserting a key places it at the end. `entries` takes a shallow snapshot of each pair.

`fromList` invokes the key function synchronously in input order. It propagates callback Errors and checks for a duplicate immediately after each successful callback. Both bulk constructors throw `rinzu.Map.DuplicateKey<K>` for the first duplicate. Its payload is `{key: K, firstIndex: Int, duplicateIndex: Int}`, with zero-based positions and the key from the duplicate entry.

## Length errors and construction

The maximum collection length is `9223372036854775807`. `rinzu.List.append` and `rinzu.List.concatAll` throw `rinzu.List.LengthOverflow` if the result would exceed that count. `rinzu.Map.insert` throws `rinzu.Map.LengthOverflow` when a new key would exceed the count. Both Errors carry `{currentLength: Int, addedLength: Int}`. In `concatAll`, these fields identify the running total and the first part that would overflow it.

Bulk operations build the output in a single pass over the elements, with a preliminary size check for `concatAll`. Map construction uses a hash index and compares keys on collisions. When the compiler can prove that the intermediate snapshots of an initially empty local are private to a synchronous loop, repeated `append` and `insert` use a growing buffer.

See [Lists and maps](../language/collections.md) for examples.
