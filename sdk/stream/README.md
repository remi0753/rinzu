# rinzu.stream

Bundled pull-based binary file streams over rinzu.io. Import directly without a dependency entry.

openFile(path) uses 64 KiB chunks; openFileWithLimit(path,maximum) accepts 1..65536 bytes. Await next(stream) for Option<ByteArray>; None marks EOF. Only one operation may use a stream at a time. Await close in finally after outstanding operations settle.

The caller supplies backpressure by requesting the next chunk. Busy, closed, I/O, and cancellation paths follow explicit physical settlement. This module does not provide generic push streams or for-await syntax.

[Guide](../../docs/libraries/io.md) · [API reference](../../docs/reference/sdk-stream.md) · [Source](src/stream.rinzu)
