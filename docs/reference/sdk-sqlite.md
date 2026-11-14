# rinzu.sqlite API reference

Generated from checked public exports and captured source comments.

Language: `rinzu-0.5` · target: `aarch64-apple-darwin` · semantics: `rinzu-language-0.5-r85`.

## rinzu.sqlite.Connection

class · [source](../../sdk/sqlite/src/sqlite.rinzu#L7).

```rinzu
class Connection
```

### field handle

[source](../../sdk/sqlite/src/sqlite.rinzu#L8).

```rinzu
const handle: ConnectionHandle
```

## rinzu.sqlite.Connection.close

Checked type: `fn(Connection)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/sqlite/src/sqlite.rinzu#L30).

```rinzu
fn close(self: Connection) -> Int
```

## rinzu.sqlite.Connection.execute

Checked type: `fn(Connection,String)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/sqlite/src/sqlite.rinzu#L22).

```rinzu
fn execute(self: Connection, sql: String) -> Int
```

## rinzu.sqlite.Connection.executeWithObserver

Checked type: `fn(Connection,String,fn($rinzu_record_1e0e3e731944d6c3da5b94095f661568)->CInt32)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/sqlite/src/sqlite.rinzu#L38).

```rinzu
fn executeWithObserver(self: Connection, sql: String, handler: fn (ObserverContext) -> CInt) -> Int
```

## rinzu.sqlite.Connection.inTransaction

Checked type: `fn(Connection)->Bool`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/sqlite/src/sqlite.rinzu#L54).

```rinzu
fn inTransaction(self: Connection) -> Bool
```

## rinzu.sqlite.Connection.isOpen

Checked type: `fn(Connection)->Bool`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/sqlite/src/sqlite.rinzu#L34).

```rinzu
fn isOpen(self: Connection) -> Bool
```

## rinzu.sqlite.Connection.open

Checked type: `fn(String)->Connection`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/sqlite/src/sqlite.rinzu#L10).

```rinzu
fn open(path: String) -> Connection
```

## rinzu.sqlite.Connection.openMemory

Checked type: `fn()->Connection`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/sqlite/src/sqlite.rinzu#L14).

```rinzu
fn openMemory() -> Connection
```

## rinzu.sqlite.Connection.prepare

Checked type: `fn(Connection,String)->Statement`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/sqlite/src/sqlite.rinzu#L18).

```rinzu
fn prepare(self: Connection, sql: String) -> Statement
```

## rinzu.sqlite.Connection.queryByteRows

Checked type: `fn(Connection,String,Int)->List<ByteArray>`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/sqlite/src/sqlite.rinzu#L58).

```rinzu
fn queryByteRows(self: Connection, sql: String, maximumRows: Int) -> List<ByteArray>
```

## rinzu.sqlite.Connection.queryBytes

Checked type: `fn(Connection,String)->ByteArray`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/sqlite/src/sqlite.rinzu#L42).

```rinzu
fn queryBytes(self: Connection, sql: String) -> ByteArray
```

## rinzu.sqlite.Connection.queryInt

Checked type: `fn(Connection,String)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/sqlite/src/sqlite.rinzu#L26).

```rinzu
fn queryInt(self: Connection, sql: String) -> Int
```

## rinzu.sqlite.Connection.queryText

Checked type: `fn(Connection,String)->String`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/sqlite/src/sqlite.rinzu#L46).

```rinzu
fn queryText(self: Connection, sql: String) -> String
```

## rinzu.sqlite.Connection.setDeadline

Checked type: `fn(Connection,Int)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/sqlite/src/sqlite.rinzu#L50).

```rinzu
fn setDeadline(self: Connection, milliseconds: Int) -> Int
```

## rinzu.sqlite.ConnectionHandle

Checked type: `ConnectionHandle`.

opaque · [source](../../sdk/sqlite/src/sqlite.rinzu#L86).

```rinzu
opaque ConnectionHandle;
```

## rinzu.sqlite.ObserverContext

Checked type: `{connection:Connection}`.

type-alias · [source](../../sdk/sqlite/src/sqlite.rinzu#L83).

```rinzu
type ObserverContext = { connection: Connection };
```

## rinzu.sqlite.Statement

class · [source](../../sdk/sqlite/src/sqlite.rinzu#L63).

```rinzu
class Statement
```

### field handle

[source](../../sdk/sqlite/src/sqlite.rinzu#L64).

```rinzu
const handle: StatementHandle
```

## rinzu.sqlite.Statement.columnInt

Checked type: `fn(Statement,Int)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/sqlite/src/sqlite.rinzu#L70).

```rinzu
fn columnInt(self: Statement, index: Int) -> Int
```

## rinzu.sqlite.Statement.finalize

Checked type: `fn(Statement)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/sqlite/src/sqlite.rinzu#L74).

```rinzu
fn finalize(self: Statement) -> Int
```

## rinzu.sqlite.Statement.isOpen

Checked type: `fn(Statement)->Bool`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/sqlite/src/sqlite.rinzu#L78).

```rinzu
fn isOpen(self: Statement) -> Bool
```

## rinzu.sqlite.Statement.step

Checked type: `fn(Statement)->Bool`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/sqlite/src/sqlite.rinzu#L66).

```rinzu
fn step(self: Statement) -> Bool
```

## rinzu.sqlite.StatementHandle

Checked type: `StatementHandle`.

opaque · [source](../../sdk/sqlite/src/sqlite.rinzu#L88).

```rinzu
opaque StatementHandle;
```

## rinzu.sqlite.allocatorCurrent

Checked type: `native()->Int`.

native-function · [source](../../sdk/sqlite/src/sqlite.rinzu#L161).

```rinzu
fn allocatorCurrent() -> Int symbol "rinzu_sqlite_allocator_current";
```

## rinzu.sqlite.allocatorPeak

Checked type: `native()->Int`.

native-function · [source](../../sdk/sqlite/src/sqlite.rinzu#L163).

```rinzu
fn allocatorPeak() -> Int symbol "rinzu_sqlite_allocator_peak";
```

## rinzu.sqlite.close

Checked type: `fn(Connection)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L226).

```rinzu
fn close(connection: Connection) -> Int
```

## rinzu.sqlite.closeOwnerConnections

Checked type: `fn()->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L230).

```rinzu
fn closeOwnerConnections() -> Int
```

## rinzu.sqlite.columnInt

Checked type: `fn(Statement,Int)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L215).

```rinzu
fn columnInt(statement: Statement, index: Int) -> Int
```

## rinzu.sqlite.enableAllocatorStatsBeforeOpen

Checked type: `fn()->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L396).

```rinzu
fn enableAllocatorStatsBeforeOpen() -> Int
```

## rinzu.sqlite.execute

Checked type: `fn(Connection,String)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L192).

```rinzu
fn execute(connection: Connection, sql: String) -> Int
```

## rinzu.sqlite.executeWithObserver

Checked type: `fn(Connection,String,fn($rinzu_record_1e0e3e731944d6c3da5b94095f661568)->CInt32)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L260).

```rinzu
fn executeWithObserver(connection: Connection, sql: String, handler: fn (ObserverContext) -> CInt) -> Int
```

## rinzu.sqlite.finalize

Checked type: `fn(Statement)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L222).

```rinzu
fn finalize(statement: Statement) -> Int
```

## rinzu.sqlite.headerVersion

Checked type: `native()->String`.

native-function · [source](../../sdk/sqlite/src/sqlite.rinzu#L171).

```rinzu
fn headerVersion() -> String symbol "rinzu_sqlite_header_version";
```

## rinzu.sqlite.inTransaction

Checked type: `fn(Connection)->Bool`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L330).

```rinzu
fn inTransaction(connection: Connection) -> Bool
```

## rinzu.sqlite.isOpen

Checked type: `fn(Connection)->Bool`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L236).

```rinzu
fn isOpen(connection: Connection) -> Bool
```

## rinzu.sqlite.isStatementOpen

Checked type: `fn(Statement)->Bool`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L248).

```rinzu
fn isStatementOpen(statement: Statement) -> Bool
```

## rinzu.sqlite.libraryVersion

Checked type: `native()->String`.

native-function · [source](../../sdk/sqlite/src/sqlite.rinzu#L169).

```rinzu
fn libraryVersion() -> String symbol "rinzu_sqlite_library_version";
```

## rinzu.sqlite.liveConnections

Checked type: `native()->Int`.

native-function · [source](../../sdk/sqlite/src/sqlite.rinzu#L157).

```rinzu
fn liveConnections() -> Int symbol "rinzu_sqlite_live_connections";
```

## rinzu.sqlite.liveStatements

Checked type: `native()->Int`.

native-function · [source](../../sdk/sqlite/src/sqlite.rinzu#L159).

```rinzu
fn liveStatements() -> Int symbol "rinzu_sqlite_live_statements";
```

## rinzu.sqlite.monotonicNanos

Checked type: `native()->Int`.

native-function · [source](../../sdk/sqlite/src/sqlite.rinzu#L167).

```rinzu
fn monotonicNanos() -> Int symbol "rinzu_sqlite_monotonic_nanos";
```

## rinzu.sqlite.open

Checked type: `fn(String)->Connection`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L176).

```rinzu
fn open(path: String) -> Connection
```

## rinzu.sqlite.openMemory

Checked type: `fn()->Connection`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L182).

```rinzu
fn openMemory() -> Connection
```

## rinzu.sqlite.ownedAllocations

Checked type: `native()->Int`.

native-function · [source](../../sdk/sqlite/src/sqlite.rinzu#L138).

```rinzu
fn ownedAllocations() -> Int symbol "rinzu_sqlite_owned_allocations";
```

## rinzu.sqlite.ownedLive

Checked type: `native()->Int`.

native-function · [source](../../sdk/sqlite/src/sqlite.rinzu#L142).

```rinzu
fn ownedLive() -> Int symbol "rinzu_sqlite_owned_live";
```

## rinzu.sqlite.ownedReleases

Checked type: `native()->Int`.

native-function · [source](../../sdk/sqlite/src/sqlite.rinzu#L140).

```rinzu
fn ownedReleases() -> Int symbol "rinzu_sqlite_owned_releases";
```

## rinzu.sqlite.prepare

Checked type: `fn(Connection,String)->Statement`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L186).

```rinzu
fn prepare(connection: Connection, sql: String) -> Statement
```

## rinzu.sqlite.queryByteRows

Checked type: `fn(Connection,String,Int)->List<ByteArray>`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L342).

```rinzu
fn queryByteRows(connection: Connection, sql: String, maximumRows: Int) -> List<ByteArray>
```

## rinzu.sqlite.queryBytes

Checked type: `fn(Connection,String)->ByteArray`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L310).

```rinzu
fn queryBytes(connection: Connection, sql: String) -> ByteArray
```

## rinzu.sqlite.queryInt

Checked type: `fn(Connection,String)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L196).

```rinzu
fn queryInt(connection: Connection, sql: String) -> Int
```

## rinzu.sqlite.queryText

Checked type: `fn(Connection,String)->String`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L314).

```rinzu
fn queryText(connection: Connection, sql: String) -> String
```

## rinzu.sqlite.retainedControlBytes

Checked type: `native()->Int`.

native-function · [source](../../sdk/sqlite/src/sqlite.rinzu#L144).

```rinzu
fn retainedControlBytes() -> Int symbol "rinzu_sqlite_retained_control_bytes";
```

## rinzu.sqlite.setDeadline

Checked type: `fn(Connection,Int)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L326).

```rinzu
fn setDeadline(connection: Connection, milliseconds: Int) -> Int
```

## rinzu.sqlite.sourceId

Checked type: `native()->String`.

native-function · [source](../../sdk/sqlite/src/sqlite.rinzu#L173).

```rinzu
fn sourceId() -> String symbol "rinzu_sqlite_source_id";
```

## rinzu.sqlite.step

Checked type: `fn(Statement)->Bool`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L203).

```rinzu
fn step(statement: Statement) -> Bool
```
