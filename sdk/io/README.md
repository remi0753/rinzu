# rinzu.io

Bundled binary file and standard-stream I/O for language 0.5 on macOS arm64. Import `rinzu.io` directly; no manifest dependency is required. Timers use `rinzu.timer`.

Use `readBytes` or `readText` with an explicit byte limit to read a whole file. `replaceBytes` and `replaceText` atomically install new contents and return a receipt containing any cleanup errors after installation.

Use async openRead/openWrite, read, write/writeAll, flush, and close. ReadChunk contains actual bytes and an EOF flag. Native operations are bounded to 64 KiB. File handles belong to their acquiring Domain and physical Owner. Await active users and close in finally; cancellation waits for physical drain.

standardInput and standardOutput acquire owned binary stream handles. rename/remove and monotonicMillis/unixMillis support filesystem work and clocks. closeOwnerFiles explicitly drains the calling Owner's resources and requires control of that Owner's complete usage scope.

liveFiles, liveBuffers, liveOperations, and retainedControlBytes measure distinct SDK quantities. Retained diagnostic controls and whole-process memory are not equivalent to live files.

[Guide](../../docs/libraries/io.md) · [API reference](../../docs/reference/sdk-io.md) · [Source](src/io.rinzu)
