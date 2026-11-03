# HTTP and TCP

The standard library contains a bounded HTTP client and nonblocking TCP. Both require explicit resource cleanup and observe the macOS arm64 Main/Owner rules.

## HTTP client

Use `rinzu.http.newClient()`, await `request`, and close the client in finally. This complete program needs a reachable HTTPS endpoint:

```rinzu
import rinzu.http;
import rinzu.stdio;

async fn main() {
    try {
        let client = rinzu.http.newClient();
        try {
            let response = await rinzu.http.request(client, "https://example.com/", byteArray(b""), false, 5000);
            rinzu.stdio.writeLine(response.status);
            rinzu.stdio.writeLine(bytesLength(response.body));
        }
        finally {
            let closed = await rinzu.http.close(client);
        }
    }
    catch problems {
        panic("HTTP request failed");
    }
}
```

The request arguments are client, URL, body, POST flag, and timeout in milliseconds. GET uses an empty body. Timeouts are 1 through 30000 ms; request and response bodies are each limited to 64 KiB. HTTPS uses certificate verification; plain HTTP is restricted to loopback. Automatic redirects, retries, and proxies are unavailable.

A Response has const `status` and `body` fields. HttpFailure carries a payload with cause, status, partial bytes, and sent count. A client cannot run overlapping operations. For details, see [HTTP declarations](../../sdk/http/src/http.rinzu).

## TCP listener and stream

`rinzu.socket.address(host,port)` constructs an Address. The supported host syntax is a literal IPv4 address. Listen on port 0 when the OS should choose an available port; read it with `localPort(listener)`.

```rinzu
// Fragment inside an async function; add try/finally around acquired resources.
let listener = await rinzu.socket.listen(rinzu.socket.address("127.0.0.1", 0), 16);

let port = rinzu.socket.localPort(listener);

let accepted = await rinzu.socket.accept(listener);

let chunk = await rinzu.socket.read(accepted, 4096);

let sent = await rinzu.socket.writeAll(accepted, chunk.data);

let streamClosed = await rinzu.socket.close(accepted);

let listenerClosed = await rinzu.socket.closeListener(listener);
```

A client uses `connect(address)`. `read` returns ReadChunk, and `write` may transfer a prefix; `writeAll` completes the requested transfer or throws a WriteFailure with transferred-byte information. Duplex operations are supported, subject to the API's per-direction ownership and busy checks. Maximum operation payloads are 64 KiB.

`connectWithTimeout`, `acceptWithTimeout`, `readWithTimeout`, `writeWithTimeout`, and `writeAllWithTimeout` make deadlines explicit. `shutdownWrite` half-closes the writing direction. Close accepted streams separately from their listener.

Cancellation waits for physical native drain. `closeOwnerSockets` is an explicit Owner-wide cleanup operation, and `statistics()` describes bounded SDK queues and resources. DNS, UDP, automatic reconnection, and a built-in HTTP server/router framework are outside these APIs.

For complete signatures and data fields, see [TCP declarations](../../sdk/socket/src/socket.rinzu). `rinzu.httpcodec` offers bounded request-line helpers, rather than a complete HTTP application framework.
