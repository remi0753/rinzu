# rinzu.stream API reference

Generated from checked public exports and captured source comments.

Language: `rinzu-0.5` · target: `aarch64-apple-darwin` · semantics: `rinzu-language-0.5-r76`.

## rinzu.stream.FileStream

class · [source](../../sdk/stream/src/stream.rinzu#L3).

```rinzu
class FileStream
```

### field file

[source](../../sdk/stream/src/stream.rinzu#L4).

```rinzu
const file: rinzu.io.File
```

### field maximum

[source](../../sdk/stream/src/stream.rinzu#L5).

```rinzu
const maximum: Int
```

### field busy

[source](../../sdk/stream/src/stream.rinzu#L6).

```rinzu
busy: Bool
```

### field state

[source](../../sdk/stream/src/stream.rinzu#L7).

```rinzu
state: Int
```

### field failure

[source](../../sdk/stream/src/stream.rinzu#L8).

```rinzu
failure: Option<Error>
```

## rinzu.stream.close

Checked type: `async(FileStream)->Int`.

function · [source](../../sdk/stream/src/stream.rinzu#L66).

```rinzu
async fn close(stream: FileStream) -> Int
```

## rinzu.stream.next

Checked type: `async(FileStream)->Option<ByteArray>`.

function · [source](../../sdk/stream/src/stream.rinzu#L28).

```rinzu
async fn next(stream: FileStream) -> Option<ByteArray>
```

## rinzu.stream.openFile

Checked type: `async(String)->FileStream`.

function · [source](../../sdk/stream/src/stream.rinzu#L15).

```rinzu
async fn openFile(path: String) -> FileStream
```

## rinzu.stream.openFileWithLimit

Checked type: `async(String,Int)->FileStream`.

function · [source](../../sdk/stream/src/stream.rinzu#L19).

```rinzu
async fn openFileWithLimit(path: String, maximum: Int) -> FileStream
```
