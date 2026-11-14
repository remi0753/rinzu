# rinzu.stream API reference

Generated from checked public exports and captured source comments.

Language: `rinzu-0.5` · target: `aarch64-apple-darwin` · semantics: `rinzu-language-0.5-r85`.

## rinzu.stream.FileStream

class · [source](../../sdk/stream/src/stream.rinzu#L4).

```rinzu
class FileStream
```

### field file

[source](../../sdk/stream/src/stream.rinzu#L5).

```rinzu
const file: rinzu.io.File
```

### field maximum

[source](../../sdk/stream/src/stream.rinzu#L6).

```rinzu
const maximum: Int
```

### field busy

[source](../../sdk/stream/src/stream.rinzu#L7).

```rinzu
busy: Bool
```

### field state

[source](../../sdk/stream/src/stream.rinzu#L8).

```rinzu
state: Int
```

### field failure

[source](../../sdk/stream/src/stream.rinzu#L9).

```rinzu
failure: Option<Error>
```

## rinzu.stream.close

Checked type: `async(FileStream) throws(Error)->Int`.

Error contract: `throws(Error)` (declared).

function · [source](../../sdk/stream/src/stream.rinzu#L67).

```rinzu
async fn close(stream: FileStream) throws(Error) -> Int
```

## rinzu.stream.next

Checked type: `async(FileStream) throws(Error)->Option<ByteArray>`.

Error contract: `throws(Error)` (declared).

function · [source](../../sdk/stream/src/stream.rinzu#L29).

```rinzu
async fn next(stream: FileStream) throws(Error) -> Option<ByteArray>
```

## rinzu.stream.openFile

Checked type: `async(String) throws(Error)->FileStream`.

Error contract: `throws(Error)` (declared).

function · [source](../../sdk/stream/src/stream.rinzu#L16).

```rinzu
async fn openFile(path: String) throws(Error) -> FileStream
```

## rinzu.stream.openFileWithLimit

Checked type: `async(String,Int) throws(Error)->FileStream`.

Error contract: `throws(Error)` (declared).

function · [source](../../sdk/stream/src/stream.rinzu#L20).

```rinzu
async fn openFileWithLimit(path: String, maximum: Int) throws(Error) -> FileStream
```
