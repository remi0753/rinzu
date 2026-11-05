# Collection API reference

Import `rinzu.list` or `rinzu.map` and qualify calls with the module name. Collection types are spelled `List<T>` and `Map<K,V>`. Map keys satisfy `Key`.

## rinzu.list

| Function | Result |
| --- | --- |
| `rinzu.list.length(List<T>)` | `Int` |
| `rinzu.list.get(List<T>, Int)` | `Option<T>` |
| `rinzu.list.append(List<T>, T)` | `List<T>` |
| `rinzu.list.set(List<T>, Int, T)` | `Option<List<T>>` |
| `rinzu.list.remove(List<T>, Int)` | `Option<List<T>>` |
| `rinzu.list.map(List<T>, fn (T) -> U)` | `List<U>` in input order |
| `rinzu.list.filter(List<T>, fn (T) -> Bool)` | Matching elements in input order |
| `rinzu.list.concatAll(List<List<T>>)` | `List<T>` in part and element order |

Negative and out-of-range indices return `Option.None`. Updates preserve existing snapshots. `map` and `filter` call their functions synchronously, once per visited element, from the start of the input. A callback Error propagates immediately with its original failure information.

## rinzu.map

| Function | Result |
| --- | --- |
| `rinzu.map.length(Map<K,V>)` | `Int` |
| `rinzu.map.get(Map<K,V>, K)` | `Option<V>` |
| `rinzu.map.insert(Map<K,V>, K, V)` | `Map<K,V>` |
| `rinzu.map.remove(Map<K,V>, K)` | `Map<K,V>` |
| `rinzu.map.keys(Map<K,V>)` | `List<K>` in insertion order |
| `rinzu.map.entries(Map<K,V>)` | `List<{key: K, value: V}>` in insertion order |
| `rinzu.map.fromList(List<T>, fn (T) -> K)` | `Map<K,T>` in input order |
| `rinzu.map.fromEntries(List<{key: K, value: V}>)` | `Map<K,V>` in input order |

A missing key returns `Option.None` from `get` and leaves the contents unchanged after `remove`. `insert` replaces a matching value at its original position and appends a new key at the end. Removing and reinserting a key places it at the end. `entries` takes a shallow snapshot of each pair.

`fromList` invokes the key function synchronously in input order. It propagates callback Errors and checks for a duplicate immediately after each successful callback. Both bulk constructors throw `rinzu.map.DuplicateKey<K>` for the first duplicate. Its payload is `{key: K, firstIndex: Int, duplicateIndex: Int}`, with zero-based positions and the key from the duplicate entry.

## Length errors and construction

The maximum collection length is `9223372036854775807`. `rinzu.list.append` and `rinzu.list.concatAll` throw `rinzu.list.LengthOverflow` if the result would exceed that count. `rinzu.map.insert` throws `rinzu.map.LengthOverflow` when a new key would exceed the count. Both Errors carry `{currentLength: Int, addedLength: Int}`. In `concatAll`, these fields identify the running total and the first part that would overflow it.

Bulk operations build the output in a single pass over the elements, with a preliminary size check for `concatAll`. Map construction uses a hash index and compares keys on collisions. When the compiler can prove that the intermediate snapshots of an initially empty local are private to a synchronous loop, repeated `append` and `insert` use a growing buffer.

See [Lists and maps](../language/collections.md) for examples.
