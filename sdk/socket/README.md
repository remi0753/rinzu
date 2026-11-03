# rinzu.socket

Bundled nonblocking TCP for language 0.5 on macOS arm64. Import directly. Addresses use literal IPv4; there is no DNS or UDP API.

Address, Listener, TcpStream, ReadChunk, WriteFailure, and Statistics describe the public surface. Use address/listen/localPort/accept for a listener, connect for a client, read/write/writeAll for transfer, shutdownWrite for half-close, and close/closeListener for cleanup. WithTimeout variants make deadlines explicit.

Read returns actual data and EOF. Write can transfer a prefix; writeAll loops until complete or throws a failure with transferred-byte information. Operations have 64 KiB payload bounds. Main Domain and physical Owner requirements apply. Duplex usage is supported with per-direction busy checks and bounded native queues.

Cancellation waits for physical drain before releasing operation state. closeOwnerSockets explicitly cleans the calling Owner's sockets after all users finish. statistics reports SDK accounting, not all OS or process memory.

[Guide](../../docs/libraries/networking.md) · [API reference](../../docs/reference/sdk-socket.md) · [Source](src/socket.rinzu)
