# rinzu.http

The Main-owned client uses macOS system libcurl. Create a client with `newClient()`, await `send(client, method, url, headers, body, timeoutMillis)`, and await `close(client)` in `finally`. A client handles one operation at a time and can be reused between requests.

`send` accepts case-sensitive HTTP method tokens up to 128 bytes. Use `header(name, text)` for a UTF-8 field value or construct a `Header` with a `ByteArray` value. Headers are ordered and may repeat. Libcurl owns request framing. `Content-Length`, `Transfer-Encoding`, `Trailer`, and `Upgrade` are reserved. `HEAD` uses an empty request body. A request or final response supports up to 64 fields and 32 KiB of header data.

For fields that are shared across requests, create a `PreparedHeaders` with `PreparedHeaders.create(headers)` and await `prepared.send(client, method, url, body, timeoutMillis)`. The immutable value can be shared by several clients and reuses the encoded fields. The client still validates framing and takes ownership of the native request data at admission.

`ResponseWithHeaders` exposes status, body, and the final response's headers in received order with lowercase names. `header(name)` returns the first matching byte value without regard to the case of the name. `headerText(name)` decodes that value as UTF-8. The list preserves repeated fields such as `Set-Cookie`. It covers final response fields, with interim fields and trailers outside its scope.

The original `request(client, url, body, post, timeoutMillis)` still provides GET and POST with a `Response` containing status and body. Timeouts are 1–30,000 ms. Request and response bodies are each limited to 64 KiB. HTTPS verifies certificates, and plain HTTP is loopback-only. `HttpFailure` carries cause, status, partial bytes, and sent count. Cancellation and close wait for physical native completion. `closeOwnerClients` requires coordination of every user on that Owner.

[Guide](../../docs/libraries/networking.md) · [API reference](../../docs/reference/sdk-http.md) · [Source](src/http.rinzu)

## HTTP application dispatch

`rinzu.http.server` provides a transport-independent HTTP application router and an HTTP/1.1 TCP listener. Create an application, register routes and middleware, and call `compile()`. Tests can use `compiled.inject(method, target)` or `injectBody` without opening a socket. A custom transport can use `http.request(method, target, headers, body)` to construct the owned request and pass it to `compiled.handle` or `compiled.dispatch`.

Use `onSync`, `getSync`, or `postSync` for a handler of type `fn (Request) -> Response`. Use `onAsync` or the async registration methods `on`, `get`, and `post` for a handler of type `fn (Request) -> Future<Response>`. Use a sync handler for work that can complete in the dispatch task. Keep operations that await I/O or timers in an async handler.

Use `useSync` for a middleware function of type `fn (Request) -> Option<Response>`. Returning `None` continues the chain and returning `Some(response)` stops it. Sync and async middleware run in registration order within each scope, from the outer scope to the route. A synchronous prefix runs in the dispatch turn, including before an async handler. An early response follows the same validation and error policy as a route response.

A server loop can call `compiled.dispatch(request)` to run a synchronous route in its current turn. It returns `Dispatch.Ready(response)` for a validated response and `Dispatch.Pending(work)` when a handler, middleware, or error policy needs an async turn. Call and await `work.run()` before advancing an ordered response stream. Each pending dispatch runs once. The call claims the request and sets route parameters immediately. A ready response has already run its handler and has no task to cancel. `handle` schedules the whole request as a Future.

Each route uses the same lookup, parameters, response validation, and error policy. A sync handler with only sync middleware runs in the turn that dispatches it. Async middleware can short-circuit or await its `Next.run()` once during its own execution. An error thrown by a middleware or handler follows the enclosing scope's `onError` policy. The policy's response is also validated.

For example, an application can register a plain response and an async route in the same router.

```rinzu-test
import rinzu.http.server as http;
import rinzu.bytes;
import rinzu.core;
import rinzu.timer;

async fn main() {
    try {
        let app = http.application();
        app.getSync("/ready", fn (input: http.Request) -> http.Response {
            return http.text(200, "ready");
        });
        app.onAsync("GET", "/later", async fn (input: http.Request) -> http.Response {
            await rinzu.timer.sleep(1);
            return http.text(200, "done");
        });
        let compiled = app.compile();
        let request = http.request("GET", "/ready", [], rinzu.bytes.from(b""));
        let response = await compiled.handle(request);
        if response.status != 200 {
            rinzu.core.panic("unexpected response status");
        }
    }
    catch problems {
        rinzu.core.panic("HTTP setup or request failed");
    }
}
```

## HTTP/1.1 listener

Bind a compiled application with `await Server.listen(compiled, host, port, options)`. Use an IPv4 literal and pass `0` to let the OS select a port. `server.port` reports the bound port. Call `await server.serve()` to accept requests, and call `await server.close()` in `finally` to release the listener even if serving never starts.

The listener decodes requests over borrowed Socket SDK buffers. It supports keep-alive and sends pipelined responses in request order. HEAD retains response metadata while suppressing the body. The transport supplies `Date`, framing and `Connection: close`. It uses the [codec's origin-form and Content-Length bounds](../httpcodec/README.md) over plain TCP.

`ServerOptions` sets the accept backlog, maximum connections, first or incomplete request deadline, keep-alive idle deadline, response write deadline, and shutdown grace period. `defaultServerOptions()` sets a backlog of 64, a cap of 48 connections, 2-second request, idle and write deadlines, and a 5-second shutdown grace period. The timeout fields accept 1–30,000 ms. Socket capacity is shared with other listeners and connections on the Owner.

`server.stop()` requests a graceful stop. A running `serve()` stops accepting, closes idle connections and finishes responses already in progress. On expiry of the grace period it requests cancellation, closes the remaining connections and waits for physical settlement. `await server.close()` also requests stop and waits for cleanup. Use `serveUntil(fn () -> Bool)` with an application stop predicate, for example one that polls `rinzu.shutdown.signalRequested()`. The predicate is checked between accepts and while the listener is idle, at most 100 ms apart during a normal idle accept. The application owns process signal registration.
