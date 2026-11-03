# rinzu.sqlite API reference

Generated from checked public exports and captured source comments.

Language: `rinzu-0.5` · target: `aarch64-apple-darwin` · semantics: `rinzu-language-0.5-r76`.

## rinzu.sqlite.Connection

class · [source](../../sdk/sqlite/src/sqlite.rinzu#L1).

```rinzu
class Connection
```

### field handle

[source](../../sdk/sqlite/src/sqlite.rinzu#L2).

```rinzu
const handle: ConnectionHandle
```

## rinzu.sqlite.Connection.close

Checked type: `fn(Connection)->Int`.

method · [source](../../sdk/sqlite/src/sqlite.rinzu#L24).

```rinzu
fn close(self: Connection) -> Int
```

## rinzu.sqlite.Connection.execute

Checked type: `fn(Connection,String)->Int`.

method · [source](../../sdk/sqlite/src/sqlite.rinzu#L16).

```rinzu
fn execute(self: Connection, sql: String) -> Int
```

## rinzu.sqlite.Connection.executeWithObserver

Checked type: `fn(Connection,String,fn($rinzu_record_1e0e3e731944d6c3da5b94095f661568)->CInt32)->Int`.

method · [source](../../sdk/sqlite/src/sqlite.rinzu#L32).

```rinzu
fn executeWithObserver(self: Connection, sql: String, handler: fn (ObserverContext) -> CInt) -> Int
```

## rinzu.sqlite.Connection.inTransaction

Checked type: `fn(Connection)->Bool`.

method · [source](../../sdk/sqlite/src/sqlite.rinzu#L48).

```rinzu
fn inTransaction(self: Connection) -> Bool
```

## rinzu.sqlite.Connection.isOpen

Checked type: `fn(Connection)->Bool`.

method · [source](../../sdk/sqlite/src/sqlite.rinzu#L28).

```rinzu
fn isOpen(self: Connection) -> Bool
```

## rinzu.sqlite.Connection.open

Checked type: `fn(String)->Connection`.

method · [source](../../sdk/sqlite/src/sqlite.rinzu#L4).

```rinzu
fn open(path: String) -> Connection
```

## rinzu.sqlite.Connection.openMemory

Checked type: `fn()->Connection`.

method · [source](../../sdk/sqlite/src/sqlite.rinzu#L8).

```rinzu
fn openMemory() -> Connection
```

## rinzu.sqlite.Connection.prepare

Checked type: `fn(Connection,String)->Statement`.

method · [source](../../sdk/sqlite/src/sqlite.rinzu#L12).

```rinzu
fn prepare(self: Connection, sql: String) -> Statement
```

## rinzu.sqlite.Connection.queryByteRows

Checked type: `fn(Connection,String,Int)->List<ByteArray>`.

method · [source](../../sdk/sqlite/src/sqlite.rinzu#L52).

```rinzu
fn queryByteRows(self: Connection, sql: String, maximumRows: Int) -> List<ByteArray>
```

## rinzu.sqlite.Connection.queryBytes

Checked type: `fn(Connection,String)->ByteArray`.

method · [source](../../sdk/sqlite/src/sqlite.rinzu#L36).

```rinzu
fn queryBytes(self: Connection, sql: String) -> ByteArray
```

## rinzu.sqlite.Connection.queryInt

Checked type: `fn(Connection,String)->Int`.

method · [source](../../sdk/sqlite/src/sqlite.rinzu#L20).

```rinzu
fn queryInt(self: Connection, sql: String) -> Int
```

## rinzu.sqlite.Connection.queryText

Checked type: `fn(Connection,String)->String`.

method · [source](../../sdk/sqlite/src/sqlite.rinzu#L40).

```rinzu
fn queryText(self: Connection, sql: String) -> String
```

## rinzu.sqlite.Connection.setDeadline

Checked type: `fn(Connection,Int)->Int`.

method · [source](../../sdk/sqlite/src/sqlite.rinzu#L44).

```rinzu
fn setDeadline(self: Connection, milliseconds: Int) -> Int
```

## rinzu.sqlite.ConnectionHandle

Checked type: `ConnectionHandle`.

opaque · [source](../../sdk/sqlite/src/sqlite.rinzu#L82).

```rinzu
opaque ConnectionHandle;
```

## rinzu.sqlite.ObserverContext

type-alias · [source](../../sdk/sqlite/src/sqlite.rinzu#L77).

```rinzu
type ObserverContext = {
    connection: Connection
};
```

## rinzu.sqlite.Statement

class · [source](../../sdk/sqlite/src/sqlite.rinzu#L57).

```rinzu
class Statement
```

### field handle

[source](../../sdk/sqlite/src/sqlite.rinzu#L58).

```rinzu
const handle: StatementHandle
```

## rinzu.sqlite.Statement.columnInt

Checked type: `fn(Statement,Int)->Int`.

method · [source](../../sdk/sqlite/src/sqlite.rinzu#L64).

```rinzu
fn columnInt(self: Statement, index: Int) -> Int
```

## rinzu.sqlite.Statement.finalize

Checked type: `fn(Statement)->Int`.

method · [source](../../sdk/sqlite/src/sqlite.rinzu#L68).

```rinzu
fn finalize(self: Statement) -> Int
```

## rinzu.sqlite.Statement.isOpen

Checked type: `fn(Statement)->Bool`.

method · [source](../../sdk/sqlite/src/sqlite.rinzu#L72).

```rinzu
fn isOpen(self: Statement) -> Bool
```

## rinzu.sqlite.Statement.step

Checked type: `fn(Statement)->Bool`.

method · [source](../../sdk/sqlite/src/sqlite.rinzu#L60).

```rinzu
fn step(self: Statement) -> Bool
```

## rinzu.sqlite.StatementHandle

Checked type: `StatementHandle`.

opaque · [source](../../sdk/sqlite/src/sqlite.rinzu#L84).

```rinzu
opaque StatementHandle;
```

## rinzu.sqlite.allocatorCurrent

Checked type: `native()->Int`.

native-function · [source](../../sdk/sqlite/src/sqlite.rinzu#L158).

```rinzu
fn allocatorCurrent() -> Int symbol "rinzu_sqlite_allocator_current";
```

## rinzu.sqlite.allocatorPeak

Checked type: `native()->Int`.

native-function · [source](../../sdk/sqlite/src/sqlite.rinzu#L160).

```rinzu
fn allocatorPeak() -> Int symbol "rinzu_sqlite_allocator_peak";
```

## rinzu.sqlite.close

Checked type: `fn(Connection)->Int`.

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L223).

```rinzu
fn close(connection: Connection) -> Int
```

## rinzu.sqlite.closeOwnerConnections

Checked type: `fn()->Int`.

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L227).

```rinzu
fn closeOwnerConnections() -> Int
```

## rinzu.sqlite.columnInt

Checked type: `fn(Statement,Int)->Int`.

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L212).

```rinzu
fn columnInt(statement: Statement, index: Int) -> Int
```

## rinzu.sqlite.enableAllocatorStatsBeforeOpen

Checked type: `fn()->Int`.

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L402).

```rinzu
fn enableAllocatorStatsBeforeOpen() -> Int
```

## rinzu.sqlite.execute

Checked type: `fn(Connection,String)->Int`.

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L189).

```rinzu
fn execute(connection: Connection, sql: String) -> Int
```

## rinzu.sqlite.executeWithObserver

Checked type: `fn(Connection,String,fn($rinzu_record_1e0e3e731944d6c3da5b94095f661568)->CInt32)->Int`.

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L257).

```rinzu
fn executeWithObserver(connection: Connection, sql: String, handler: fn (ObserverContext) -> CInt) -> Int
```

## rinzu.sqlite.finalize

Checked type: `fn(Statement)->Int`.

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L219).

```rinzu
fn finalize(statement: Statement) -> Int
```

## rinzu.sqlite.headerVersion

Checked type: `native()->String`.

native-function · [source](../../sdk/sqlite/src/sqlite.rinzu#L168).

```rinzu
fn headerVersion() -> String symbol "rinzu_sqlite_header_version";
```

## rinzu.sqlite.inTransaction

Checked type: `fn(Connection)->Bool`.

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L336).

```rinzu
fn inTransaction(connection: Connection) -> Bool
```

## rinzu.sqlite.isOpen

Checked type: `fn(Connection)->Bool`.

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L233).

```rinzu
fn isOpen(connection: Connection) -> Bool
```

## rinzu.sqlite.isStatementOpen

Checked type: `fn(Statement)->Bool`.

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L245).

```rinzu
fn isStatementOpen(statement: Statement) -> Bool
```

## rinzu.sqlite.libraryVersion

Checked type: `native()->String`.

native-function · [source](../../sdk/sqlite/src/sqlite.rinzu#L166).

```rinzu
fn libraryVersion() -> String symbol "rinzu_sqlite_library_version";
```

## rinzu.sqlite.liveConnections

Checked type: `native()->Int`.

native-function · [source](../../sdk/sqlite/src/sqlite.rinzu#L154).

```rinzu
fn liveConnections() -> Int symbol "rinzu_sqlite_live_connections";
```

## rinzu.sqlite.liveStatements

Checked type: `native()->Int`.

native-function · [source](../../sdk/sqlite/src/sqlite.rinzu#L156).

```rinzu
fn liveStatements() -> Int symbol "rinzu_sqlite_live_statements";
```

## rinzu.sqlite.monotonicNanos

Checked type: `native()->Int`.

native-function · [source](../../sdk/sqlite/src/sqlite.rinzu#L164).

```rinzu
fn monotonicNanos() -> Int symbol "rinzu_sqlite_monotonic_nanos";
```

## rinzu.sqlite.open

Checked type: `fn(String)->Connection`.

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L173).

```rinzu
fn open(path: String) -> Connection
```

## rinzu.sqlite.openMemory

Checked type: `fn()->Connection`.

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L179).

```rinzu
fn openMemory() -> Connection
```

## rinzu.sqlite.ownedAllocations

Checked type: `native()->Int`.

native-function · [source](../../sdk/sqlite/src/sqlite.rinzu#L134).

```rinzu
fn ownedAllocations() -> Int symbol "rinzu_sqlite_owned_allocations";
```

## rinzu.sqlite.ownedLive

Checked type: `native()->Int`.

native-function · [source](../../sdk/sqlite/src/sqlite.rinzu#L138).

```rinzu
fn ownedLive() -> Int symbol "rinzu_sqlite_owned_live";
```

## rinzu.sqlite.ownedReleases

Checked type: `native()->Int`.

native-function · [source](../../sdk/sqlite/src/sqlite.rinzu#L136).

```rinzu
fn ownedReleases() -> Int symbol "rinzu_sqlite_owned_releases";
```

## rinzu.sqlite.prepare

Checked type: `fn(Connection,String)->Statement`.

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L183).

```rinzu
fn prepare(connection: Connection, sql: String) -> Statement
```

## rinzu.sqlite.queryByteRows

Checked type: `fn(Connection,String,Int)->List<ByteArray>`.

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L348).

```rinzu
fn queryByteRows(connection: Connection, sql: String, maximumRows: Int) -> List<ByteArray>
```

## rinzu.sqlite.queryBytes

Checked type: `fn(Connection,String)->ByteArray`.

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L316).

```rinzu
fn queryBytes(connection: Connection, sql: String) -> ByteArray
```

## rinzu.sqlite.queryInt

Checked type: `fn(Connection,String)->Int`.

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L193).

```rinzu
fn queryInt(connection: Connection, sql: String) -> Int
```

## rinzu.sqlite.queryText

Checked type: `fn(Connection,String)->String`.

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L320).

```rinzu
fn queryText(connection: Connection, sql: String) -> String
```

## rinzu.sqlite.retainedControlBytes

Checked type: `native()->Int`.

native-function · [source](../../sdk/sqlite/src/sqlite.rinzu#L140).

```rinzu
fn retainedControlBytes() -> Int symbol "rinzu_sqlite_retained_control_bytes";
```

## rinzu.sqlite.setDeadline

Checked type: `fn(Connection,Int)->Int`.

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L332).

```rinzu
fn setDeadline(connection: Connection, milliseconds: Int) -> Int
```

## rinzu.sqlite.sourceId

Checked type: `native()->String`.

native-function · [source](../../sdk/sqlite/src/sqlite.rinzu#L170).

```rinzu
fn sourceId() -> String symbol "rinzu_sqlite_source_id";
```

## rinzu.sqlite.step

Checked type: `fn(Statement)->Bool`.

function · [source](../../sdk/sqlite/src/sqlite.rinzu#L200).

```rinzu
fn step(statement: Statement) -> Bool
```
