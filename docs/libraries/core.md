# Core API reference

The compiler supplies the modules below. Import the module to use its qualified name or give it an alias with `as`. Imports do not introduce unqualified members. Existing unqualified names and their qualified `rinzu.core` spellings remain available during migration.

| Module | Public functions |
| --- | --- |
| `rinzu.error` | `create`, `message`, `code`, `context`, `withContext`, `withCause`, `cause`, `cleanup`, `sourceFileId`, `sourceStartByte`, `sourceEndByte`, `nativeStatus` |
| `rinzu.string` | `length`, `get`, `slice`, `find`, `split`, `utf8ByteLength`, `encodeUtf8`, `decodeUtf8` |
| `rinzu.bytes` | `from`, `byteLength`, `length`, `get`, `slice`, `concat`, `concatAll`, `copy`, `xor`, `findAny`, `findNotAny`, `containsAny`, `allIn`, `trim`, `lowerAscii` |
| `rinzu.number` | `parseInt`, `formatInt`, `bitAnd`, `bitOr`, `bitXor`, `bitNot`, `shiftLeft`, `shiftRight`, `wrappingAdd`, `wrappingSub`, `wrappingMul`, `checkedAdd`, `checkedSub`, `checkedMul`, `abs`, `sqrt`, `isNaN` |
| `rinzu.future` | `isReady`, `cancel`, `timeout` |
| `rinzu.native` | `completed`, `transferred`, `checkStatus` |
| `rinzu.shutdown` | `request`, alongside the existing SDK exports |
| `rinzu.core` | `panic` |

Use `rinzu.io.readSize` for packaged resource size and `rinzu.process.setExitStatus` for process exit status. The migration table below lists the compatibility spellings.

List and Map operations are documented in the [collection API reference](collections.md).

## Text and binary data

| Function | Result |
| --- | --- |
| `rinzu.string.length(String)` / `rinzu.string.utf8ByteLength(String)` | Int scalar count / CSize byte count |
| `rinzu.string.get(String, Int)` / `rinzu.string.slice(String, Int, Int)` | Option<String> |
| `rinzu.string.find(String, String, Int)` | Option<Int> |
| `rinzu.string.split(String, String)` | List<String> |
| `rinzu.number.parseInt(String)` / `rinzu.number.formatInt(Int)` | Int, throwing on invalid input / String |
| `rinzu.string.encodeUtf8(String)` / `rinzu.string.decodeUtf8(ByteArray)` | ByteArray / String, throwing on invalid UTF-8 |
| `rinzu.bytes.from(Bytes(N))` / `rinzu.bytes.from(List<CUInt8>)` | ByteArray |
| `rinzu.bytes.length(ByteArray)` / `rinzu.bytes.byteLength(Bytes(N) or ByteArray)` | Int / CSize |
| `rinzu.bytes.get(ByteArray, Int)` | Option<CUInt8> |
| `rinzu.bytes.slice(ByteArray, Int, Int)` | Option<ByteArray> |
| `rinzu.bytes.concat(ByteArray, ByteArray)` / `rinzu.bytes.concatAll(List<ByteArray>)` | ByteArray |
| `rinzu.bytes.copy(ByteArray)` / `rinzu.bytes.xor(ByteArray, CUInt8)` | ByteArray |

Binary scanning helpers accept a byte-set ByteArray. `rinzu.bytes.findAny(data,set,start)` and `rinzu.bytes.findNotAny(data,set,start)` return Option<Int>. `rinzu.bytes.containsAny(data,set)` and `rinzu.bytes.allIn(data,set)` return Bool. `rinzu.bytes.trim(data,set)` trims matching bytes at both ends. `rinzu.bytes.lowerAscii(data)` changes ASCII uppercase bytes and preserves other bytes.

See [Strings and bytes](../language/strings-and-bytes.md) for scalar indexing, raw text, strict parsing, and binary lengths.

## Numeric operations

In `rinzu.number`, `bitAnd/Or/Xor/Not`, `shiftLeft/Right`, `wrappingAdd/Sub/Mul`, `checkedAdd/Sub/Mul`, `abs`, `sqrt`, and `isNaN` follow the concrete scalar type. Checked overflow and invalid shifts throw Error. See [Operators](../language/operators.md).

## Errors

`rinzu.error.create(code: Int, message: String)` constructs a generic Error. `rinzu.error.message(problem: Error) nothrow -> String` returns the same value as `problem.message` for both generic and named Errors, including Errors with type arguments. The compatibility names are `error` and `errorMessage`. The message accessor evaluates its argument once and propagates any Error from that evaluation.

| `rinzu.error` accessor | Result |
| --- | --- |
| `code(Error)` / `context(Error)` | `Option<Int>` / `Option<String>`. Generic Errors have both values, including an empty context. Named Errors return `None`. |
| `cause(Error)` | `Option<Error>`, with the same value as `.cause`. |
| `nativeStatus(Error)` | `Option<Int>`, present when the Error carries native status. |
| `sourceFileId(Error)` / `sourceStartByte(Error)` / `sourceEndByte(Error)` | Optional low-level positions. For display, use `.source`. |
| `cleanup(Error)` | `List<Error>` stored on this Error. Catch the failure list to handle every Error from the current execution. |
| `withContext(Error, String)` / `withCause(Error, Error)` | `Option<Error>`. On success, a copy retains source and trace. Both updates return `None` for a named target, and `withCause` also returns `None` for a named cause. Existing bounds on cause graphs still apply. |

`rinzu.core.panic(message: String)` signals an unrecoverable failure and still follows required settlement. See [Errors](../language/errors.md).

## Tasks and native services

`rinzu.future.isReady`, `rinzu.future.cancel`, and `rinzu.future.timeout` operate on supported Main Futures. `rinzu.shutdown.request` and `rinzu.process.setExitStatus` control application shutdown. See [Async](../language/async.md).

`rinzu.native.completed(opaque)` and `rinzu.native.transferred(opaque)` are completion bridges for SDK implementations. Their inputs must use the supported native completion protocols. `rinzu.native.checkStatus(status: Int, context: String)` returns a successful status or throws for native failure. Applications should use declared Error types when recovery depends on a specific condition.

`rinzu.io.readSize` reads the size of a packaged resource. Use the file APIs in `rinzu.io` for ordinary file I/O.

## Compatibility names

The old source names still resolve during migration. Use the module on the right in new imports. When migrating `errorCode`, `errorContext`, `errorWithContext`, or `errorWithCause`, handle the optional result at the call site.

| Old name | Canonical API |
| --- | --- |
| `panic` | `rinzu.core.panic` |
| `error` | `rinzu.error.create` |
| `errorMessage` | `rinzu.error.message` |
| `errorCode` | `rinzu.error.code` |
| `errorContext` | `rinzu.error.context` |
| `errorWithContext` | `rinzu.error.withContext` |
| `errorWithCause` | `rinzu.error.withCause` |
| `errorCause` | `rinzu.error.cause` |
| `errorCleanup` | `rinzu.error.cleanup` |
| `errorSourceFile` | `rinzu.error.sourceFileId` |
| `errorSourceStart` | `rinzu.error.sourceStartByte` |
| `errorSourceEnd` | `rinzu.error.sourceEndByte` |
| `errorNativeStatus` | `rinzu.error.nativeStatus` |
| `stringLength` | `rinzu.string.length` |
| `stringGet` | `rinzu.string.get` |
| `stringSlice` | `rinzu.string.slice` |
| `stringFind` | `rinzu.string.find` |
| `stringSplit` | `rinzu.string.split` |
| `utf8ByteLength` | `rinzu.string.utf8ByteLength` |
| `utf8Encode` | `rinzu.string.encodeUtf8` |
| `utf8Decode` | `rinzu.string.decodeUtf8` |
| `byteArray` | `rinzu.bytes.from` |
| `byteLength` | `rinzu.bytes.byteLength` |
| `bytesLength` | `rinzu.bytes.length` |
| `bytesGet` | `rinzu.bytes.get` |
| `bytesSlice` | `rinzu.bytes.slice` |
| `bytesConcat` | `rinzu.bytes.concat` |
| `bytesConcatAll` | `rinzu.bytes.concatAll` |
| `bytesCopy` | `rinzu.bytes.copy` |
| `bytesXor` | `rinzu.bytes.xor` |
| `bytesFindAny` | `rinzu.bytes.findAny` |
| `bytesFindNotAny` | `rinzu.bytes.findNotAny` |
| `bytesContainsAny` | `rinzu.bytes.containsAny` |
| `bytesAllIn` | `rinzu.bytes.allIn` |
| `bytesTrim` | `rinzu.bytes.trim` |
| `bytesLowerAscii` | `rinzu.bytes.lowerAscii` |
| `parseInt` | `rinzu.number.parseInt` |
| `formatInt` | `rinzu.number.formatInt` |
| `bitAnd` | `rinzu.number.bitAnd` |
| `bitOr` | `rinzu.number.bitOr` |
| `bitXor` | `rinzu.number.bitXor` |
| `bitNot` | `rinzu.number.bitNot` |
| `shiftLeft` | `rinzu.number.shiftLeft` |
| `shiftRight` | `rinzu.number.shiftRight` |
| `wrappingAdd` | `rinzu.number.wrappingAdd` |
| `wrappingSub` | `rinzu.number.wrappingSub` |
| `wrappingMul` | `rinzu.number.wrappingMul` |
| `checkedAdd` | `rinzu.number.checkedAdd` |
| `checkedSub` | `rinzu.number.checkedSub` |
| `checkedMul` | `rinzu.number.checkedMul` |
| `mathAbs` | `rinzu.number.abs` |
| `mathSqrt` | `rinzu.number.sqrt` |
| `floatIsNan` | `rinzu.number.isNaN` |
| `futureReady` | `rinzu.future.isReady` |
| `cancelFuture` | `rinzu.future.cancel` |
| `timeoutFuture` | `rinzu.future.timeout` |
| `nativeCompleted` | `rinzu.native.completed` |
| `nativeTransferred` | `rinzu.native.transferred` |
| `nativeStatus` | `rinzu.native.checkStatus` |
| `requestShutdown` | `rinzu.shutdown.request` |
| `readResourceSize` | `rinzu.io.readSize` |
| `setProcessExitStatus` | `rinzu.process.setExitStatus` |

The compiler and ABI define the operation identities in [core.rs](../../crates/compiler/src/core.rs) and [data_operation.rs](../../crates/abi/src/data_operation.rs).
