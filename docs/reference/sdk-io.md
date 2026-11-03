# rinzu.io API reference

Generated from checked public exports and captured source comments.

Language: `rinzu-0.5` · target: `aarch64-apple-darwin` · semantics: `rinzu-language-0.5-r76`.

## rinzu.io.File

Checked type: `File`.

opaque · [source](../../sdk/io/src/io.rinzu#L6).

```rinzu
opaque File;
```

## rinzu.io.ReadChunk

class · [source](../../sdk/io/src/io.rinzu#L67).

```rinzu
class ReadChunk
```

### field data

[source](../../sdk/io/src/io.rinzu#L68).

```rinzu
const data: ByteArray
```

### field eof

[source](../../sdk/io/src/io.rinzu#L69).

```rinzu
const eof: Bool
```

## rinzu.io.close

Checked type: `async(File)->Int`.

function · [source](../../sdk/io/src/io.rinzu#L316).

```rinzu
async fn close(file: File) -> Int
```

## rinzu.io.closeOwnerFiles

Checked type: `async()->Int`.

function · [source](../../sdk/io/src/io.rinzu#L320).

```rinzu
async fn closeOwnerFiles() -> Int
```

## rinzu.io.flush

Checked type: `async(File)->Int`.

function · [source](../../sdk/io/src/io.rinzu#L312).

```rinzu
async fn flush(file: File) -> Int
```

## rinzu.io.installTemporary

Checked type: `async(File,String)->Int`.

function · [source](../../sdk/io/src/io.rinzu#L158).

```rinzu
async fn installTemporary(file: File, destination: String) -> Int
```

Atomically rename an open temporary file, preserving it after close.

## rinzu.io.liveBuffers

Checked type: `native()->Int`.

native-function · [source](../../sdk/io/src/io.rinzu#L58).

```rinzu
fn liveBuffers() -> Int symbol "rinzu_io_live_buffers";
```

## rinzu.io.liveFiles

Checked type: `native()->Int`.

native-function · [source](../../sdk/io/src/io.rinzu#L56).

```rinzu
fn liveFiles() -> Int symbol "rinzu_io_live_files";
```

## rinzu.io.liveOperations

Checked type: `native()->Int`.

native-function · [source](../../sdk/io/src/io.rinzu#L60).

```rinzu
fn liveOperations() -> Int symbol "rinzu_io_live_operations";
```

## rinzu.io.monotonicMillis

Checked type: `native()->Int`.

native-function · [source](../../sdk/io/src/io.rinzu#L52).

```rinzu
fn monotonicMillis() -> Int symbol "rinzu_io_monotonic";
```

## rinzu.io.openRead

Checked type: `async(String)->File`.

function · [source](../../sdk/io/src/io.rinzu#L127).

```rinzu
async fn openRead(path: String) -> File
```

## rinzu.io.openTemporary

Checked type: `async(String)->File`.

function · [source](../../sdk/io/src/io.rinzu#L137).

```rinzu
async fn openTemporary(template: String) -> File
```

Create a unique regular file with a template ending in XXXXXX.
close removes it unless installTemporary has physically completed.

## rinzu.io.openWrite

Checked type: `async(String)->File`.

function · [source](../../sdk/io/src/io.rinzu#L131).

```rinzu
async fn openWrite(path: String) -> File
```

## rinzu.io.read

Checked type: `async(File,Int)->ReadChunk`.

function · [source](../../sdk/io/src/io.rinzu#L200).

```rinzu
async fn read(file: File, maximum: Int) -> ReadChunk
```

## rinzu.io.readSize

Checked type: `fn(String)->Future<Int>`.

function · [source](../../sdk/io/src/io.rinzu#L1).

```rinzu
fn readSize(path: String) -> Future<Int>
```

## rinzu.io.remove

Checked type: `async(String)->Int`.

function · [source](../../sdk/io/src/io.rinzu#L357).

```rinzu
async fn remove(path: String) -> Int
```

## rinzu.io.rename

Checked type: `async(String,String)->Int`.

function · [source](../../sdk/io/src/io.rinzu#L353).

```rinzu
async fn rename(from: String, to: String) -> Int
```

## rinzu.io.retainedControlBytes

Checked type: `native()->Int`.

native-function · [source](../../sdk/io/src/io.rinzu#L62).

```rinzu
fn retainedControlBytes() -> Int symbol "rinzu_io_retained_controls";
```

## rinzu.io.standardInput

Checked type: `async()->File`.

function · [source](../../sdk/io/src/io.rinzu#L192).

```rinzu
async fn standardInput() -> File
```

## rinzu.io.standardOutput

Checked type: `async()->File`.

function · [source](../../sdk/io/src/io.rinzu#L196).

```rinzu
async fn standardOutput() -> File
```

## rinzu.io.unixMillis

Checked type: `native()->Int`.

native-function · [source](../../sdk/io/src/io.rinzu#L54).

```rinzu
fn unixMillis() -> Int symbol "rinzu_io_unix";
```

## rinzu.io.write

Checked type: `async(File,ByteArray)->Int`.

function · [source](../../sdk/io/src/io.rinzu#L234).

```rinzu
async fn write(file: File, data: ByteArray) -> Int
```

## rinzu.io.writeAll

Checked type: `async(File,ByteArray)->Int`.

function · [source](../../sdk/io/src/io.rinzu#L259).

```rinzu
async fn writeAll(file: File, data: ByteArray) -> Int
```
