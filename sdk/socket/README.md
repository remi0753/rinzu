# rinzu.socket

Bundled nonblocking TCP for language 0.5 on macOS arm64. Import directly. Addresses use literal IPv4; there is no DNS or UDP API.

Address, Listener, TcpStream, ReadChunk, WriteFailure, and Statistics describe the public surface. Use address/listen/localPort/accept for a listener, connect for a client, read/write/writeAll for transfer, shutdownWrite for half-close, and close/closeListener for cleanup. WithTimeout variants make deadlines explicit.

Read returns actual data and EOF. Write can transfer a prefix; writeAll loops until complete or throws a failure with transferred-byte information. Operations have 64 KiB payload bounds. Main Domain and physical Owner requirements apply. Duplex usage is supported with per-direction busy checks and bounded native queues.

Use `withRead(stream, maximum, consume)` or `withReadTimeout(stream, maximum, milliseconds, consume)` to process received bytes synchronously before releasing the receive operation. The callback runs once on a successful read, including EOF. `readLength(view)` reports the initialized length, with zero meaning EOF. `readByte(view, index)` and `findReadByte(view, byte, start, end)` inspect bytes in place. `copyRead(view, start, end)` returns an owned copy of the half-open range. That copy can cross an await and outlive the callback.

`ReadView` is an opaque, Owner-bound handle. Calls that use an expired view or an invalid range fail. Retaining the handle or returning a Future from the callback does not extend the view's lifetime. An error thrown by the callback propagates after the completed read is released. Cancellation before the callback drains the physical operation before cleanup.

The read Future starts on the Owner. If the initial nonblocking receive succeeds, the result is delivered during that turn without reserving a reactor slot or creating a native completion Future. A would-block receive transfers its buffer to the pending operation. If a control poll interrupts delivery after bytes have been consumed, the SDK closes the stream before releasing the buffer.

Native consumers can declare arguments of type `rinzu.socket.ReadAccess` and `rinzu.socket.ReadView`, obtain the accessor from `readAccess()`, and use `rinzu_native_read_view` from `rinzu/native_read_view.h` during a synchronous native call. The returned descriptor is read-only and expires with the callback.

`tryWrite(stream, data)` makes at most one nonblocking send in the current Owner turn. The return value is the accepted prefix length. Zero means empty input or that the send could not proceed immediately. An I/O error throws `WriteFailure` with zero bytes accepted by that call. The call keeps no pending operation or copy. For an ordered stream, serialize the accepted prefix and any subsequent write of the unsent suffix before starting another message. Only the subsequent asynchronous write can be cancelled. A prefix already accepted by the OS remains sent, so close the stream if cancellation leaves the application message incomplete.

An Owner write that finishes in one send uses the normal completion scheduling without allocating a socket operation or staging buffer. The runtime can schedule the already completed receipt directly when the SDK awaits it once, without creating a separate completion Future. A write that still needs I/O owns its unsent bytes and preserves its accepted prefix. Cancellation waits for physical drain before releasing pending operation state. closeOwnerSockets explicitly cleans the calling Owner's sockets after all users finish. statistics reports SDK accounting, not all OS or process memory.

Registered readiness operations can share an already bounded reactor wait. The reactor can also notify one runtime after publishing multiple completions for it. Each operation retains its own result and physical completion.

[Guide](../../docs/libraries/networking.md) · [API reference](../../docs/reference/sdk-socket.md) · [Source](src/socket.rinzu)
