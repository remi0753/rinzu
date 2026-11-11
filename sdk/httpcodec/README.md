# rinzu.httpcodec

Bundled HTTP/1.1 request-line validation and incremental request decoding for applications that supply their own transport and request handlers.

Create one `Decoder` per input stream and call `close()` in a `finally` block. Use `feed(data, start)` with owned bytes or `feedRead(view, start)` inside a Socket SDK read callback. Both return one owned `RequestFrame` on success. The frame's `end` is the exclusive offset in the current input. Feed the same input again from that offset to decode a pipelined request. `Option.None` means the decoder has consumed the supplied range and needs more input. At transport EOF, call `end()` to check for a truncated message.

For borrowed input, the decoder reads contiguous framing directly. It copies the unfinished head or body ranges that must survive to the next feed. Completed frames own their request head and body and can be passed to asynchronous handlers. Header names are lowercased and values are trimmed and decoded as UTF-8. `frame.head.closing` reports a `close` token in a Connection field.

`Decoder.withHeadCache(bytes)` optionally keeps one validated request head and its decoded fields. An exact byte match reuses the head. The key includes the request line, all header lines and the terminating blank line. Bodies are decoded separately for every request. The cache budget is 0–4,096 bytes. A nonzero `frame.head.version` identifies the decoded head within that decoder, so callers can also reuse work derived from its fields. `Decoder.create()` leaves the cache disabled.

Limits are 8,192 bytes per line, 32 KiB for the header section including its terminating CRLF, 64 header fields, and a 64 KiB body. The decoder accepts origin-form HTTP/1.1 requests with one Host and an optional Content-Length. Malformed requests and truncation raise `InvalidRequest` with an HTTP status and leave the decoder in a terminal state. Transfer-Encoding alone yields 501. Ambiguous framing yields 400.

`requestLine(data)` remains available for callers that only need to validate and decode one request line without its trailing CRLF.

[Networking guide](../../docs/libraries/networking.md) · [API reference](../../docs/reference/sdk-httpcodec.md) · [Source](src/httpcodec.rinzu)
