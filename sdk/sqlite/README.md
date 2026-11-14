# rinzu.sqlite

Official source SDK for SQLite connections, statements, copied results, observers, and deadlines. Import it directly from a matching toolchain bundle, or declare a local path to the SDK. Connection and Statement are managed classes wrapping opaque native handles; handles have no members or constructors.

## Ownership and cleanup

Acquire, use, and close resources on the same physical Owner. Await child users, finalize statements, and close connections in finally. GC does not destroy the native payload. A busy connection can be retried after its statements settle; a statement is closed even when finalize reports an Error.

closeOwnerConnections drains the calling Owner's connection/statement resources, including an acquisition that did not deliver a source handle. It rejects active use before closing anything. Coordinate every user on that Owner before an Owner-wide drain.

## Results and background work

queryText and queryBytes copy one-row/one-column TEXT or BLOB values into managed snapshots, bounded to 65536 bytes. Text uses strict UTF-8 and retains NUL. Invalid SQL input, row/type/size mismatches, and invalid UTF-8 throw Error. SQLite-owned result storage is released by the source cleanup path.

queryByteRows returns bounded one-column BLOB snapshots: a row limit of 1..256 and a combined 64 KiB bound. setDeadline accepts 0 to disable or 1..60000 ms; progress/busy checks do not guarantee interruption of arbitrary VFS/OS blocking. inTransaction supports explicit rollback policy.

Synchronous queries block their caller. For background work, open, query, catch ordinary failures, and close entirely inside a Worker, then return a transferable success snapshot. Never transfer a connection.

## Callbacks and measurements

The executeWithObserver handler receives an ObserverContext with connection. It runs only within exec on the same Owner and follows its checked fallback. Reentrant connection use or close from the observer is rejected.

liveConnections/liveStatements, owned allocation/release/live counters, and retainedControlBytes describe different resource classes. enableAllocatorStatsBeforeOpen is an explicit physical-Main startup setting, before SQLite initialization. Invalid/unavailable allocator statistics are not zero usage.

[SDK guide](../../docs/libraries/sdk.md) · [API reference](../../docs/reference/sdk-sqlite.md) · [Source](src/sqlite.rinzu)
