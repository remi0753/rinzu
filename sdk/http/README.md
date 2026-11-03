# rinzu.http

Bundled Main-owned HTTP client using macOS system libcurl. newClient creates a client; await request(client,url,body,post,timeoutMillis), then await close in finally. A client cannot run overlapping operations.

GET uses an empty body; POST accepts binary data. Timeout is 1..30000 ms. Request and response bodies are each limited to 64 KiB. HTTPS verifies certificates; plain HTTP is loopback-only. Redirects, retries, and proxies are unavailable.

Response exposes status and body. HttpFailure carries cause, status, partial bytes, and sent count in its payload. Cancellation and close wait for physical native completion; acceptance occurs inside protected cleanup scopes. closeOwnerClients requires coordination of every user on that Owner.

[Guide](../../docs/libraries/networking.md) · [API reference](../../docs/reference/sdk-http.md) · [Source](src/http.rinzu)
