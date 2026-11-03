# Core API reference

Core operations are built in and available without imports. The signatures below use T, K, and V to describe the concrete types inferred from arguments. They are grouped by purpose; follow the language chapters for examples and capability rules.

## Collections

| Function | Result |
| --- | --- |
| `listLength(List<T>)` | Int |
| `listGet(List<T>, Int)` | Option<T> |
| `listAppend(List<T>, T)` | List<T> |
| `listSet(List<T>, Int, T)` / `listRemove(List<T>, Int)` | Option<List<T>> |
| `mapLength(Map<K,V>)` | Int |
| `mapGet(Map<K,V>, K)` | Option<V> |
| `mapInsert(Map<K,V>, K, V)` / `mapRemove(Map<K,V>, K)` | Map<K,V> |
| `mapKeys(Map<K,V>)` | List<K>, in insertion order |

Invalid list indices and absent map lookups return None. See [Lists and maps](../language/collections.md) for immutable update and cost rules.

## Text and binary data

| Function | Result |
| --- | --- |
| `stringLength(String)` / `utf8ByteLength(String)` | Int scalar count / CSize byte count |
| `stringGet(String, Int)` / `stringSlice(String, Int, Int)` | Option<String> |
| `stringFind(String, String, Int)` | Option<Int> |
| `stringSplit(String, String)` | List<String> |
| `parseInt(String)` / `formatInt(Int)` | Int, throwing on invalid input / String |
| `utf8Encode(String)` / `utf8Decode(ByteArray)` | ByteArray / String, throwing on invalid UTF-8 |
| `byteArray(Bytes(N))` / `byteArray(List<CUInt8>)` | ByteArray |
| `bytesLength(ByteArray)` / `byteLength(Bytes(N) or ByteArray)` | Int / CSize |
| `bytesGet(ByteArray, Int)` | Option<CUInt8> |
| `bytesSlice(ByteArray, Int, Int)` | Option<ByteArray> |
| `bytesConcat(ByteArray, ByteArray)` / `bytesConcatAll(List<ByteArray>)` | ByteArray |
| `bytesCopy(ByteArray)` / `bytesXor(ByteArray, CUInt8)` | ByteArray |

Binary scanning helpers accept a byte-set ByteArray: `bytesFindAny(data,set,start)` and `bytesFindNotAny(data,set,start)` return Option<Int>; `bytesContainsAny(data,set)` and `bytesAllIn(data,set)` return Bool. `bytesTrim(data,set)` trims matching bytes at both ends. `bytesLowerAscii(data)` changes ASCII uppercase bytes and preserves other bytes.

See [Strings and bytes](../language/strings-and-bytes.md) for scalar indexing, raw text, strict parsing, and binary lengths.

## Numeric operations

`bitAnd/Or/Xor/Not`, `shiftLeft/Right`, `wrappingAdd/Sub/Mul`, `checkedAdd/Sub/Mul`, `mathAbs`, `mathSqrt`, and `floatIsNan` follow the concrete scalar type. Checked overflow and invalid shifts throw Error. See [Operators](../language/operators.md).

## Errors

`error(code: Int,message: String)` constructs a generic Error. `errorCode`, `errorMessage`, and `errorContext` inspect it. `errorWithContext` and `errorWithCause` return values with additional metadata. `errorCause` returns Option<Error>; `errorCleanup` returns List<Error>.

`errorSourceFile`, `errorSourceStart`, `errorSourceEnd`, and `errorNativeStatus` expose optional low-level metadata. For source-oriented reporting of user Errors, prefer their `source` location. `nativeStatus(status: Int,context: String)` returns a successful status or throws for native failure. Applications should use declared Error types when recovery depends on a specific condition.

`panic(message: String)` is an unrecoverable failure that still follows required settlement. See [Errors](../language/errors.md).

## Tasks and native services

`futureReady`, `cancelFuture`, `timeoutFuture`, `requestShutdown`, and `setProcessExitStatus` control supported Main tasks and application shutdown. See [Async](../language/async.md).

`nativeCompleted(opaque)` and `nativeTransferred(opaque)` are completion bridges used by SDK implementations. They are not generic conversions of arbitrary native calls into async functions. `readResourceSize` supports the toolchain's packaged resource mechanism; ordinary file I/O should use `rinzu.io`.

Canonical operation names and signatures are implemented in [core.rs](../../crates/compiler/src/core.rs) and [data_operation.rs](../../crates/abi/src/data_operation.rs).
