# rinzu.socket API reference

Generated from checked public exports and captured source comments.

Language: `rinzu-0.5` · target: `aarch64-apple-darwin` · semantics: `rinzu-language-0.5-r76`.

## rinzu.socket.Address

class · [source](../../sdk/socket/src/socket.rinzu#L79).

```rinzu
class Address
```

### field host

[source](../../sdk/socket/src/socket.rinzu#L80).

```rinzu
const host: String
```

### field port

[source](../../sdk/socket/src/socket.rinzu#L81).

```rinzu
const port: Int
```

## rinzu.socket.Listener

Checked type: `Listener`.

opaque · [source](../../sdk/socket/src/socket.rinzu#L4).

```rinzu
opaque Listener;
```

## rinzu.socket.ReadChunk

class · [source](../../sdk/socket/src/socket.rinzu#L84).

```rinzu
class ReadChunk
```

### field data

[source](../../sdk/socket/src/socket.rinzu#L85).

```rinzu
const data: ByteArray
```

### field eof

[source](../../sdk/socket/src/socket.rinzu#L86).

```rinzu
const eof: Bool
```

## rinzu.socket.Statistics

class · [source](../../sdk/socket/src/socket.rinzu#L395).

```rinzu
class Statistics
```

### field listeners

[source](../../sdk/socket/src/socket.rinzu#L396).

```rinzu
const listeners: Int
```

### field streams

[source](../../sdk/socket/src/socket.rinzu#L397).

```rinzu
const streams: Int
```

### field handleSlots

[source](../../sdk/socket/src/socket.rinzu#L398).

```rinzu
const handleSlots: Int
```

### field operations

[source](../../sdk/socket/src/socket.rinzu#L399).

```rinzu
const operations: Int
```

### field dataOperations

[source](../../sdk/socket/src/socket.rinzu#L400).

```rinzu
const dataOperations: Int
```

### field controlOperations

[source](../../sdk/socket/src/socket.rinzu#L401).

```rinzu
const controlOperations: Int
```

### field buffers

[source](../../sdk/socket/src/socket.rinzu#L402).

```rinzu
const buffers: Int
```

### field bufferBytes

[source](../../sdk/socket/src/socket.rinzu#L403).

```rinzu
const bufferBytes: Int
```

### field peakBufferBytes

[source](../../sdk/socket/src/socket.rinzu#L404).

```rinzu
const peakBufferBytes: Int
```

### field controlBytes

[source](../../sdk/socket/src/socket.rinzu#L405).

```rinzu
const controlBytes: Int
```

### field retainedControlBytes

[source](../../sdk/socket/src/socket.rinzu#L406).

```rinzu
const retainedControlBytes: Int
```

### field eventThreads

[source](../../sdk/socket/src/socket.rinzu#L407).

```rinzu
const eventThreads: Int
```

### field keventCalls

[source](../../sdk/socket/src/socket.rinzu#L408).

```rinzu
const keventCalls: Int
```

### field readinessEvents

[source](../../sdk/socket/src/socket.rinzu#L409).

```rinzu
const readinessEvents: Int
```

### field userWakes

[source](../../sdk/socket/src/socket.rinzu#L410).

```rinzu
const userWakes: Int
```

### field pollTimeouts

[source](../../sdk/socket/src/socket.rinzu#L411).

```rinzu
const pollTimeouts: Int
```

### field staleEvents

[source](../../sdk/socket/src/socket.rinzu#L412).

```rinzu
const staleEvents: Int
```

### field readCalls

[source](../../sdk/socket/src/socket.rinzu#L413).

```rinzu
const readCalls: Int
```

### field writeCalls

[source](../../sdk/socket/src/socket.rinzu#L414).

```rinzu
const writeCalls: Int
```

### field partialWrites

[source](../../sdk/socket/src/socket.rinzu#L415).

```rinzu
const partialWrites: Int
```

### field copyBytes

[source](../../sdk/socket/src/socket.rinzu#L416).

```rinzu
const copyBytes: Int
```

### field allocations

[source](../../sdk/socket/src/socket.rinzu#L417).

```rinzu
const allocations: Int
```

### field receivedBytes

[source](../../sdk/socket/src/socket.rinzu#L418).

```rinzu
const receivedBytes: Int
```

### field sentBytes

[source](../../sdk/socket/src/socket.rinzu#L419).

```rinzu
const sentBytes: Int
```

### field pendingOperations

[source](../../sdk/socket/src/socket.rinzu#L420).

```rinzu
const pendingOperations: Int
```

### field finishWaitOperations

[source](../../sdk/socket/src/socket.rinzu#L421).

```rinzu
const finishWaitOperations: Int
```

## rinzu.socket.TcpStream

Checked type: `TcpStream`.

opaque · [source](../../sdk/socket/src/socket.rinzu#L6).

```rinzu
opaque TcpStream;
```

## rinzu.socket.WriteFailure

error · [source](../../sdk/socket/src/socket.rinzu#L89).

```rinzu
error WriteFailure
```

### field payload

[source](../../sdk/socket/src/socket.rinzu#L90).

```rinzu
payload: WriteFailureData
```

## rinzu.socket.WriteFailureData

class · [source](../../sdk/socket/src/socket.rinzu#L93).

```rinzu
class WriteFailureData
```

### field cause

[source](../../sdk/socket/src/socket.rinzu#L94).

```rinzu
const cause: Error
```

### field transferred

[source](../../sdk/socket/src/socket.rinzu#L95).

```rinzu
const transferred: Int
```

## rinzu.socket.accept

Checked type: `async(Listener)->TcpStream`.

function · [source](../../sdk/socket/src/socket.rinzu#L243).

```rinzu
async fn accept(listener: Listener) -> TcpStream
```

## rinzu.socket.acceptWithTimeout

Checked type: `async(Listener,Int)->TcpStream`.

function · [source](../../sdk/socket/src/socket.rinzu#L247).

```rinzu
async fn acceptWithTimeout(listener: Listener, milliseconds: Int) -> TcpStream
```

## rinzu.socket.address

Checked type: `fn(String,Int)->Address`.

function · [source](../../sdk/socket/src/socket.rinzu#L126).

```rinzu
fn address(host: String, port: Int) -> Address
```

## rinzu.socket.close

Checked type: `async(TcpStream)->Int`.

function · [source](../../sdk/socket/src/socket.rinzu#L374).

```rinzu
async fn close(stream: TcpStream) -> Int
```

## rinzu.socket.closeListener

Checked type: `async(Listener)->Int`.

function · [source](../../sdk/socket/src/socket.rinzu#L378).

```rinzu
async fn closeListener(listener: Listener) -> Int
```

## rinzu.socket.closeOwnerSockets

Checked type: `async()->Int`.

function · [source](../../sdk/socket/src/socket.rinzu#L382).

```rinzu
async fn closeOwnerSockets() -> Int
```

## rinzu.socket.connect

Checked type: `async(Address)->TcpStream`.

function · [source](../../sdk/socket/src/socket.rinzu#L232).

```rinzu
async fn connect(value: Address) -> TcpStream
```

## rinzu.socket.connectWithTimeout

Checked type: `async(Address,Int)->TcpStream`.

function · [source](../../sdk/socket/src/socket.rinzu#L236).

```rinzu
async fn connectWithTimeout(value: Address, milliseconds: Int) -> TcpStream
```

## rinzu.socket.listen

Checked type: `async(Address,Int)->Listener`.

function · [source](../../sdk/socket/src/socket.rinzu#L205).

```rinzu
async fn listen(value: Address, backlog: Int) -> Listener
```

## rinzu.socket.localPort

Checked type: `fn(Listener)->Int`.

function · [source](../../sdk/socket/src/socket.rinzu#L216).

```rinzu
fn localPort(listener: Listener) -> Int
```

## rinzu.socket.read

Checked type: `async(TcpStream,Int)->ReadChunk`.

function · [source](../../sdk/socket/src/socket.rinzu#L289).

```rinzu
async fn read(stream: TcpStream, maximum: Int) -> ReadChunk
```

## rinzu.socket.readWithTimeout

Checked type: `async(TcpStream,Int,Int)->ReadChunk`.

function · [source](../../sdk/socket/src/socket.rinzu#L293).

```rinzu
async fn readWithTimeout(stream: TcpStream, maximum: Int, milliseconds: Int) -> ReadChunk
```

## rinzu.socket.shutdownWrite

Checked type: `async(TcpStream)->Int`.

function · [source](../../sdk/socket/src/socket.rinzu#L370).

```rinzu
async fn shutdownWrite(stream: TcpStream) -> Int
```

## rinzu.socket.statistics

Checked type: `fn()->Statistics`.

function · [source](../../sdk/socket/src/socket.rinzu#L424).

```rinzu
fn statistics() -> Statistics
```

## rinzu.socket.write

Checked type: `async(TcpStream,ByteArray)->Int`.

function · [source](../../sdk/socket/src/socket.rinzu#L330).

```rinzu
async fn write(stream: TcpStream, data: ByteArray) -> Int
```

## rinzu.socket.writeAll

Checked type: `async(TcpStream,ByteArray)->Int`.

function · [source](../../sdk/socket/src/socket.rinzu#L341).

```rinzu
async fn writeAll(stream: TcpStream, data: ByteArray) -> Int
```

## rinzu.socket.writeAllWithTimeout

Checked type: `async(TcpStream,ByteArray,Int)->Int`.

function · [source](../../sdk/socket/src/socket.rinzu#L345).

```rinzu
async fn writeAllWithTimeout(stream: TcpStream, data: ByteArray, milliseconds: Int) -> Int
```

## rinzu.socket.writeWithTimeout

Checked type: `async(TcpStream,ByteArray,Int)->Int`.

function · [source](../../sdk/socket/src/socket.rinzu#L334).

```rinzu
async fn writeWithTimeout(stream: TcpStream, data: ByteArray, milliseconds: Int) -> Int
```
