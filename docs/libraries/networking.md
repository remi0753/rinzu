# HTTP and TCP

The standard library contains HTTP application dispatch, an HTTP/1.1 listener, a bounded HTTP client, and nonblocking TCP. Listeners and clients require explicit cleanup and observe the macOS arm64 Main/Owner rules.

## HTTP client

Use `rinzu.http.newClient()`, await `send`, and close the client in `finally`. This complete program needs a reachable HTTPS endpoint.

```rinzu
import rinzu.http;
import rinzu.bytes;
import rinzu.core;
import rinzu.stdio;

async fn main() {
    try {
        let client = rinzu.http.newClient();
        try {
            let response = await rinzu.http.send(
                client, "GET", "https://example.com/", [rinzu.http.header("Accept", "text/html")],
                rinzu.bytes.from(b""), 5000
            );
            rinzu.stdio.writeLine(response.status);
            rinzu.stdio.writeLine(rinzu.bytes.length(response.body));
        }
        finally {
            let closed = await rinzu.http.close(client);
        }
    }
    catch problems {
        rinzu.core.panic("HTTP request failed");
    }
}
```

`send` accepts a method token and a list of `Header` values. `header(name, text)` constructs a UTF-8 value, and the `Header` constructor accepts raw bytes. `ResponseWithHeaders` returns status, body, and final response headers, including repeats. Use `response.header(name)` for bytes or `response.headerText(name)` for UTF-8. The original `request(client, url, body, post, timeoutMillis)` remains available for GET and POST.

Timeouts are 1–30,000 ms. Request and response bodies are each limited to 64 KiB. HTTPS verifies certificates, and plain HTTP is restricted to loopback. `HttpFailure` carries cause, status, partial bytes, and sent count. Use separate clients for overlapping operations. [HTTP SDK](../../sdk/http/README.md)

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

Cancellation waits for physical native drain. `closeOwnerSockets` is an explicit Owner-wide cleanup operation, and `statistics()` describes bounded SDK queues and resources. DNS, UDP, and automatic reconnection are outside these socket APIs.

For synchronous processing of received bytes, use `withRead` or `withReadTimeout`. The callback borrows a read view and can retain any needed range with `copyRead`. For complete signatures and lifetime rules, see the [Socket SDK](../../sdk/socket/README.md).

## Incremental HTTP decoding

Call `rinzu.httpcodec.Decoder.create()` for each connection and `close()` when that connection ends. Feed an owned byte array or call `feedRead` inside a Socket SDK read callback. Process complete frames in order and use each frame's `end` to continue within the input. The returned frames own their fields, so they can be used after the callback, including in an async handler. The [HTTP codec SDK](../../sdk/httpcodec/README.md) lists framing limits and error statuses.

## HTTP application dispatch

Use `rinzu.http.server.application()` to register routes and compile them before receiving requests. Use `getSync`, `postSync`, or `onSync` for synchronous handlers. Use `onAsync` or the async methods `on`, `get`, and `post` for handlers returning a Future. Both paths use the same routing, middleware and error policy.

Pass a parsed request to `await compiled.handle(request)`. To test routes in memory, use `await compiled.inject(method, target)`. See the [HTTP dispatch SDK](../../sdk/http/README.md#http-application-dispatch) for the handler and transport contracts.

## HTTP/1.1 server

Bind a compiled app with `await rinzu.http.server.Server.listen(compiled, host, port, options)`. Use `rinzu.http.server.defaultServerOptions()` or provide a `ServerOptions` value. The listener manages keep-alive, ordered pipelined responses, and request, idle and write deadlines. Await `server.serve()` and call `await server.close()` in `finally`. To shut it down, call `server.stop()` or use `serveUntil` with the application's stop predicate. The server closes idle connections and grants in-flight responses a configurable grace period. [HTTP/1.1 listener](../../sdk/http/README.md#http11-listener) · [Inventory example](../../examples/inventory/README.md)
