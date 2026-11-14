# rinzu.socket API reference

Generated from checked public exports and captured source comments.

Language: `rinzu-0.5` · target: `aarch64-apple-darwin` · semantics: `rinzu-language-0.5-r85`.

## rinzu.socket.Address

class · [source](../../sdk/socket/src/socket.rinzu#L107).

```rinzu
class Address
```

### field host

[source](../../sdk/socket/src/socket.rinzu#L108).

```rinzu
const host: String
```

### field port

[source](../../sdk/socket/src/socket.rinzu#L109).

```rinzu
const port: Int
```

## rinzu.socket.Listener

Checked type: `Listener`.

opaque · [source](../../sdk/socket/src/socket.rinzu#L9).

```rinzu
opaque Listener;
```

## rinzu.socket.ReadAccess

Checked type: `ReadAccess`.

opaque · [source](../../sdk/socket/src/socket.rinzu#L17).

```rinzu
opaque ReadAccess;
```

## rinzu.socket.ReadChunk

class · [source](../../sdk/socket/src/socket.rinzu#L112).

```rinzu
class ReadChunk
```

### field data

[source](../../sdk/socket/src/socket.rinzu#L113).

```rinzu
const data: ByteArray
```

### field eof

[source](../../sdk/socket/src/socket.rinzu#L114).

```rinzu
const eof: Bool
```

## rinzu.socket.ReadView

Checked type: `ReadView`.

opaque · [source](../../sdk/socket/src/socket.rinzu#L19).

```rinzu
opaque ReadView;
```

## rinzu.socket.Statistics

class · [source](../../sdk/socket/src/socket.rinzu#L543).

```rinzu
class Statistics
```

### field listeners

[source](../../sdk/socket/src/socket.rinzu#L544).

```rinzu
const listeners: Int
```

### field streams

[source](../../sdk/socket/src/socket.rinzu#L545).

```rinzu
const streams: Int
```

### field handleSlots

[source](../../sdk/socket/src/socket.rinzu#L546).

```rinzu
const handleSlots: Int
```

### field operations

[source](../../sdk/socket/src/socket.rinzu#L547).

```rinzu
const operations: Int
```

### field dataOperations

[source](../../sdk/socket/src/socket.rinzu#L548).

```rinzu
const dataOperations: Int
```

### field controlOperations

[source](../../sdk/socket/src/socket.rinzu#L549).

```rinzu
const controlOperations: Int
```

### field buffers

[source](../../sdk/socket/src/socket.rinzu#L550).

```rinzu
const buffers: Int
```

### field bufferBytes

[source](../../sdk/socket/src/socket.rinzu#L551).

```rinzu
const bufferBytes: Int
```

### field peakBufferBytes

[source](../../sdk/socket/src/socket.rinzu#L552).

```rinzu
const peakBufferBytes: Int
```

### field controlBytes

[source](../../sdk/socket/src/socket.rinzu#L553).

```rinzu
const controlBytes: Int
```

### field retainedControlBytes

[source](../../sdk/socket/src/socket.rinzu#L554).

```rinzu
const retainedControlBytes: Int
```

### field eventThreads

[source](../../sdk/socket/src/socket.rinzu#L555).

```rinzu
const eventThreads: Int
```

### field keventCalls

[source](../../sdk/socket/src/socket.rinzu#L556).

```rinzu
const keventCalls: Int
```

### field readinessEvents

[source](../../sdk/socket/src/socket.rinzu#L557).

```rinzu
const readinessEvents: Int
```

### field userWakes

[source](../../sdk/socket/src/socket.rinzu#L558).

```rinzu
const userWakes: Int
```

### field pollTimeouts

[source](../../sdk/socket/src/socket.rinzu#L559).

```rinzu
const pollTimeouts: Int
```

### field staleEvents

[source](../../sdk/socket/src/socket.rinzu#L560).

```rinzu
const staleEvents: Int
```

### field readCalls

[source](../../sdk/socket/src/socket.rinzu#L561).

```rinzu
const readCalls: Int
```

### field writeCalls

[source](../../sdk/socket/src/socket.rinzu#L562).

```rinzu
const writeCalls: Int
```

### field partialWrites

[source](../../sdk/socket/src/socket.rinzu#L563).

```rinzu
const partialWrites: Int
```

### field copyBytes

[source](../../sdk/socket/src/socket.rinzu#L564).

```rinzu
const copyBytes: Int
```

### field allocations

[source](../../sdk/socket/src/socket.rinzu#L565).

```rinzu
const allocations: Int
```

### field receivedBytes

[source](../../sdk/socket/src/socket.rinzu#L566).

```rinzu
const receivedBytes: Int
```

### field sentBytes

[source](../../sdk/socket/src/socket.rinzu#L567).

```rinzu
const sentBytes: Int
```

### field pendingOperations

[source](../../sdk/socket/src/socket.rinzu#L568).

```rinzu
const pendingOperations: Int
```

### field finishWaitOperations

[source](../../sdk/socket/src/socket.rinzu#L569).

```rinzu
const finishWaitOperations: Int
```

## rinzu.socket.TcpStream

Checked type: `TcpStream`.

opaque · [source](../../sdk/socket/src/socket.rinzu#L11).

```rinzu
opaque TcpStream;
```

## rinzu.socket.WriteFailure

error · [source](../../sdk/socket/src/socket.rinzu#L117).

```rinzu
error WriteFailure
```

### field payload

[source](../../sdk/socket/src/socket.rinzu#L118).

```rinzu
payload: WriteFailureData
```

## rinzu.socket.WriteFailureData

class · [source](../../sdk/socket/src/socket.rinzu#L121).

```rinzu
class WriteFailureData
```

### field cause

[source](../../sdk/socket/src/socket.rinzu#L122).

```rinzu
const cause: Error
```

### field transferred

[source](../../sdk/socket/src/socket.rinzu#L123).

```rinzu
const transferred: Int
```

## rinzu.socket.accept

Checked type: `async(Listener)->TcpStream`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/socket/src/socket.rinzu#L271).

```rinzu
async fn accept(listener: Listener) -> TcpStream
```

## rinzu.socket.acceptWithTimeout

Checked type: `async(Listener,Int)->TcpStream`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/socket/src/socket.rinzu#L275).

```rinzu
async fn acceptWithTimeout(listener: Listener, milliseconds: Int) -> TcpStream
```

## rinzu.socket.address

Checked type: `fn(String,Int)->Address`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/socket/src/socket.rinzu#L154).

```rinzu
fn address(host: String, port: Int) -> Address
```

## rinzu.socket.close

Checked type: `async(TcpStream)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/socket/src/socket.rinzu#L522).

```rinzu
async fn close(stream: TcpStream) -> Int
```

## rinzu.socket.closeListener

Checked type: `async(Listener)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/socket/src/socket.rinzu#L526).

```rinzu
async fn closeListener(listener: Listener) -> Int
```

## rinzu.socket.closeOwnerSockets

Checked type: `async()->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/socket/src/socket.rinzu#L530).

```rinzu
async fn closeOwnerSockets() -> Int
```

## rinzu.socket.connect

Checked type: `async(Address)->TcpStream`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/socket/src/socket.rinzu#L260).

```rinzu
async fn connect(value: Address) -> TcpStream
```

## rinzu.socket.connectWithTimeout

Checked type: `async(Address,Int)->TcpStream`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/socket/src/socket.rinzu#L264).

```rinzu
async fn connectWithTimeout(value: Address, milliseconds: Int) -> TcpStream
```

## rinzu.socket.copyRead

Checked type: `fn(ReadView,Int,Int)->ByteArray`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/socket/src/socket.rinzu#L428).

```rinzu
fn copyRead(view: ReadView, start: Int, end: Int) -> ByteArray
```

## rinzu.socket.findReadByte

Checked type: `fn(ReadView,Int,Int,Int)->Option<Int>`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/socket/src/socket.rinzu#L417).

```rinzu
fn findReadByte(view: ReadView, byte: Int, start: Int, end: Int) -> Option<Int>
```

## rinzu.socket.listen

Checked type: `async(Address,Int)->Listener`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/socket/src/socket.rinzu#L233).

```rinzu
async fn listen(value: Address, backlog: Int) -> Listener
```

## rinzu.socket.localPort

Checked type: `fn(Listener)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/socket/src/socket.rinzu#L244).

```rinzu
fn localPort(listener: Listener) -> Int
```

## rinzu.socket.read

Checked type: `async(TcpStream,Int)->ReadChunk`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/socket/src/socket.rinzu#L330).

```rinzu
async fn read(stream: TcpStream, maximum: Int) -> ReadChunk
```

## rinzu.socket.readAccess

Checked type: `native()->ReadAccess`.

native-function · [source](../../sdk/socket/src/socket.rinzu#L64).

```rinzu
fn readAccess() -> ReadAccess symbol "rinzu_socket_read_access";
```

## rinzu.socket.readByte

Checked type: `fn(ReadView,Int)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/socket/src/socket.rinzu#L409).

```rinzu
fn readByte(view: ReadView, index: Int) -> Int
```

## rinzu.socket.readLength

Checked type: `fn(ReadView)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/socket/src/socket.rinzu#L401).

```rinzu
fn readLength(view: ReadView) -> Int
```

## rinzu.socket.readWithTimeout

Checked type: `async(TcpStream,Int,Int)->ReadChunk`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/socket/src/socket.rinzu#L334).

```rinzu
async fn readWithTimeout(stream: TcpStream, maximum: Int, milliseconds: Int) -> ReadChunk
```

## rinzu.socket.shutdownWrite

Checked type: `async(TcpStream)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/socket/src/socket.rinzu#L518).

```rinzu
async fn shutdownWrite(stream: TcpStream) -> Int
```

## rinzu.socket.statistics

Checked type: `fn()->Statistics`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/socket/src/socket.rinzu#L572).

```rinzu
fn statistics() -> Statistics
```

## rinzu.socket.tryWrite

Checked type: `fn(TcpStream,ByteArray)->Int`.

Error contract: `throws(WriteFailure)` (inferred).

function · [source](../../sdk/socket/src/socket.rinzu#L432).

```rinzu
fn tryWrite(stream: TcpStream, data: ByteArray) -> Int
```

## rinzu.socket.withRead

Checked type: `fn(TcpStream,Int,fn(ReadView)->T)->Future<T>`.

Error contract: inferred for each specialization.

function · [source](../../sdk/socket/src/socket.rinzu#L393).

```rinzu
fn withRead<T>(stream: TcpStream, maximum: Int, consume: fn (ReadView) -> T) -> Future<T>
```

## rinzu.socket.withReadTimeout

Checked type: `fn(TcpStream,Int,Int,fn(ReadView)->T)->Future<T>`.

Error contract: inferred for each specialization.

function · [source](../../sdk/socket/src/socket.rinzu#L397).

```rinzu
fn withReadTimeout<T>(stream: TcpStream, maximum: Int, milliseconds: Int, consume: fn (ReadView) -> T) -> Future<T>
```

## rinzu.socket.write

Checked type: `async(TcpStream,ByteArray)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/socket/src/socket.rinzu#L478).

```rinzu
async fn write(stream: TcpStream, data: ByteArray) -> Int
```

## rinzu.socket.writeAll

Checked type: `async(TcpStream,ByteArray)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/socket/src/socket.rinzu#L489).

```rinzu
async fn writeAll(stream: TcpStream, data: ByteArray) -> Int
```

## rinzu.socket.writeAllWithTimeout

Checked type: `async(TcpStream,ByteArray,Int)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/socket/src/socket.rinzu#L493).

```rinzu
async fn writeAllWithTimeout(stream: TcpStream, data: ByteArray, milliseconds: Int) -> Int
```

## rinzu.socket.writeWithTimeout

Checked type: `async(TcpStream,ByteArray,Int)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/socket/src/socket.rinzu#L482).

```rinzu
async fn writeWithTimeout(stream: TcpStream, data: ByteArray, milliseconds: Int) -> Int
```
