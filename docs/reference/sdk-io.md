# rinzu.io API reference

Generated from checked public exports and captured source comments.

Language: `rinzu-0.5` · target: `aarch64-apple-darwin` · semantics: `rinzu-language-0.5-r85`.

[Machine-readable API v4 snapshot](../../../../../../private/tmp/rinzu-whole-io.RIRozuzD/io-api.json).

## rinzu.io.File

Checked type: `File`.

opaque · [source](../../sdk/io/src/io.rinzu#L13).

```rinzu
opaque File;
```

## rinzu.io.FileFailure

Checked type: `{path:String,phase:FilePhase,status:Option<Int>,temporaryName:Option<String>}`.

type-alias · [source](../../sdk/io/src/io.rinzu#L408).

```rinzu
type FileFailure = {
    path: String,
    phase: FilePhase,
    status: Option<Int>,
    temporaryName: Option<String>
};
```

## rinzu.io.FileOperationFailed

error · [source](../../sdk/io/src/io.rinzu#L415).

```rinzu
error FileOperationFailed
```

### field payload

[source](../../sdk/io/src/io.rinzu#L416).

```rinzu
payload: FileFailure
```

## rinzu.io.FilePhase

enum · [source](../../sdk/io/src/io.rinzu#L391).

```rinzu
enum FilePhase
```

### variant ValidatePath

[source](../../sdk/io/src/io.rinzu#L392).

```rinzu
ValidatePath
```

### variant Submit

[source](../../sdk/io/src/io.rinzu#L393).

```rinzu
Submit
```

### variant OpenFile

[source](../../sdk/io/src/io.rinzu#L394).

```rinzu
OpenFile
```

### variant Read

[source](../../sdk/io/src/io.rinzu#L395).

```rinzu
Read
```

### variant OpenDirectory

[source](../../sdk/io/src/io.rinzu#L396).

```rinzu
OpenDirectory
```

### variant InspectDestination

[source](../../sdk/io/src/io.rinzu#L397).

```rinzu
InspectDestination
```

### variant CreateTemporary

[source](../../sdk/io/src/io.rinzu#L398).

```rinzu
CreateTemporary
```

### variant Write

[source](../../sdk/io/src/io.rinzu#L399).

```rinzu
Write
```

### variant CopyMetadata

[source](../../sdk/io/src/io.rinzu#L400).

```rinzu
CopyMetadata
```

### variant Flush

[source](../../sdk/io/src/io.rinzu#L401).

```rinzu
Flush
```

### variant Install

[source](../../sdk/io/src/io.rinzu#L402).

```rinzu
Install
```

### variant CloseFile

[source](../../sdk/io/src/io.rinzu#L403).

```rinzu
CloseFile
```

### variant RemoveTemporary

[source](../../sdk/io/src/io.rinzu#L404).

```rinzu
RemoveTemporary
```

### variant CloseDirectory

[source](../../sdk/io/src/io.rinzu#L405).

```rinzu
CloseDirectory
```

## rinzu.io.InstallUncertain

error · [source](../../sdk/io/src/io.rinzu#L431).

```rinzu
error InstallUncertain
```

### field payload

[source](../../sdk/io/src/io.rinzu#L432).

```rinzu
payload: FileFailure
```

## rinzu.io.InvalidFileUtf8

error · [source](../../sdk/io/src/io.rinzu#L427).

```rinzu
error InvalidFileUtf8
```

### field payload

[source](../../sdk/io/src/io.rinzu#L428).

```rinzu
payload: { path: String }
```

## rinzu.io.InvalidReadLimit

error · [source](../../sdk/io/src/io.rinzu#L419).

```rinzu
error InvalidReadLimit
```

### field payload

[source](../../sdk/io/src/io.rinzu#L420).

```rinzu
payload: { maxBytes: Int }
```

## rinzu.io.ReadChunk

class · [source](../../sdk/io/src/io.rinzu#L74).

```rinzu
class ReadChunk
```

### field data

[source](../../sdk/io/src/io.rinzu#L75).

```rinzu
const data: ByteArray
```

### field eof

[source](../../sdk/io/src/io.rinzu#L76).

```rinzu
const eof: Bool
```

## rinzu.io.ReadLimitExceeded

error · [source](../../sdk/io/src/io.rinzu#L423).

```rinzu
error ReadLimitExceeded
```

### field payload

[source](../../sdk/io/src/io.rinzu#L424).

```rinzu
payload: { path: String, maxBytes: Int }
```

## rinzu.io.ReplaceReceipt

Checked type: `{bytesWritten:Int,cleanupErrors:List<FileOperationFailed>}`.

type-alias · [source](../../sdk/io/src/io.rinzu#L435).

```rinzu
type ReplaceReceipt = {
    bytesWritten: Int,
    cleanupErrors: List<FileOperationFailed>
};
```

## rinzu.io.close

Checked type: `async(File)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/io/src/io.rinzu#L323).

```rinzu
async fn close(file: File) -> Int
```

## rinzu.io.closeOwnerFiles

Checked type: `async()->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/io/src/io.rinzu#L327).

```rinzu
async fn closeOwnerFiles() -> Int
```

## rinzu.io.flush

Checked type: `async(File)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/io/src/io.rinzu#L319).

```rinzu
async fn flush(file: File) -> Int
```

## rinzu.io.installTemporary

Checked type: `async(File,String)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/io/src/io.rinzu#L165).

```rinzu
async fn installTemporary(file: File, destination: String) -> Int
```

Atomically rename an open temporary file, preserving it after close.

## rinzu.io.liveBuffers

Checked type: `native()->Int`.

native-function · [source](../../sdk/io/src/io.rinzu#L65).

```rinzu
fn liveBuffers() -> Int symbol "rinzu_io_live_buffers";
```

## rinzu.io.liveFiles

Checked type: `native()->Int`.

native-function · [source](../../sdk/io/src/io.rinzu#L63).

```rinzu
fn liveFiles() -> Int symbol "rinzu_io_live_files";
```

## rinzu.io.liveOperations

Checked type: `native()->Int`.

native-function · [source](../../sdk/io/src/io.rinzu#L67).

```rinzu
fn liveOperations() -> Int symbol "rinzu_io_live_operations";
```

## rinzu.io.monotonicMillis

Checked type: `native()->Int`.

native-function · [source](../../sdk/io/src/io.rinzu#L59).

```rinzu
fn monotonicMillis() -> Int symbol "rinzu_io_monotonic";
```

## rinzu.io.openRead

Checked type: `async(String)->File`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/io/src/io.rinzu#L134).

```rinzu
async fn openRead(path: String) -> File
```

## rinzu.io.openTemporary

Checked type: `async(String)->File`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/io/src/io.rinzu#L144).

```rinzu
async fn openTemporary(template: String) -> File
```

Create a unique regular file with a template ending in XXXXXX.
close removes it unless installTemporary has physically completed.

## rinzu.io.openWrite

Checked type: `async(String)->File`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/io/src/io.rinzu#L138).

```rinzu
async fn openWrite(path: String) -> File
```

## rinzu.io.read

Checked type: `async(File,Int)->ReadChunk`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/io/src/io.rinzu#L207).

```rinzu
async fn read(file: File, maximum: Int) -> ReadChunk
```

## rinzu.io.readBytes

Checked type: `async(String,Int) throws(FileOperationFailed,InvalidReadLimit,ReadLimitExceeded)->ByteArray`.

Error contract: `throws(FileOperationFailed, InvalidReadLimit, ReadLimitExceeded)` (declared).

function · [source](../../sdk/io/src/io.rinzu#L634).

```rinzu
async fn readBytes(path: String, maxBytes: Int) throws(InvalidReadLimit, ReadLimitExceeded, FileOperationFailed)
    -> ByteArray
```

Read a regular file completely, up to the required byte limit.

## rinzu.io.readSize

Checked type: `fn(String)->Future<Int>`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/io/src/io.rinzu#L8).

```rinzu
fn readSize(path: String) -> Future<Int>
```

## rinzu.io.readText

Checked type: `async(String,Int) throws(FileOperationFailed,InvalidFileUtf8,InvalidReadLimit,ReadLimitExceeded)->String`.

Error contract: `throws(FileOperationFailed, InvalidFileUtf8, InvalidReadLimit, ReadLimitExceeded)` (declared).

function · [source](../../sdk/io/src/io.rinzu#L647).

```rinzu
async fn readText(path: String, maxBytes: Int) throws(
    InvalidReadLimit,
    ReadLimitExceeded,
    FileOperationFailed,
    InvalidFileUtf8
) -> String
```

Read and strictly decode a regular file as UTF-8, preserving its BOM, NULs and newlines.

## rinzu.io.remove

Checked type: `async(String)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/io/src/io.rinzu#L364).

```rinzu
async fn remove(path: String) -> Int
```

## rinzu.io.rename

Checked type: `async(String,String)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/io/src/io.rinzu#L360).

```rinzu
async fn rename(from: String, to: String) -> Int
```

## rinzu.io.replaceBytes

Checked type: `async(String,ByteArray) throws(FileOperationFailed,InstallUncertain)->{bytesWritten:Int,cleanupErrors:List<FileOperationFailed>}`.

Error contract: `throws(FileOperationFailed, InstallUncertain)` (declared).

function · [source](../../sdk/io/src/io.rinzu#L693).

```rinzu
async fn replaceBytes(path: String, data: ByteArray) throws(FileOperationFailed, InstallUncertain) -> ReplaceReceipt
```

Atomically install the new bytes. The receipt includes any post-install cleanup errors.

## rinzu.io.replaceText

Checked type: `async(String,String) throws(FileOperationFailed,InstallUncertain)->{bytesWritten:Int,cleanupErrors:List<FileOperationFailed>}`.

Error contract: `throws(FileOperationFailed, InstallUncertain)` (declared).

function · [source](../../sdk/io/src/io.rinzu#L754).

```rinzu
async fn replaceText(path: String, text: String) throws(FileOperationFailed, InstallUncertain) -> ReplaceReceipt
```

Encode UTF-8 and atomically install the resulting bytes.

## rinzu.io.retainedControlBytes

Checked type: `native()->Int`.

native-function · [source](../../sdk/io/src/io.rinzu#L69).

```rinzu
fn retainedControlBytes() -> Int symbol "rinzu_io_retained_controls";
```

## rinzu.io.standardInput

Checked type: `async()->File`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/io/src/io.rinzu#L199).

```rinzu
async fn standardInput() -> File
```

## rinzu.io.standardOutput

Checked type: `async()->File`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/io/src/io.rinzu#L203).

```rinzu
async fn standardOutput() -> File
```

## rinzu.io.unixMillis

Checked type: `native()->Int`.

native-function · [source](../../sdk/io/src/io.rinzu#L61).

```rinzu
fn unixMillis() -> Int symbol "rinzu_io_unix";
```

## rinzu.io.write

Checked type: `async(File,ByteArray)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/io/src/io.rinzu#L241).

```rinzu
async fn write(file: File, data: ByteArray) -> Int
```

## rinzu.io.writeAll

Checked type: `async(File,ByteArray)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/io/src/io.rinzu#L266).

```rinzu
async fn writeAll(file: File, data: ByteArray) -> Int
```
