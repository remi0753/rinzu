# rinzu.log

Additional SDK for bounded asynchronous numeric-context logging. Await writes so the application applies backpressure. A row is limited to 128 bytes; the application chooses its numeric context and does not implicitly expose payloads or raw Error values.

Native operations and cleanup follow the SDK's physical Owner and completion rules. Use the generated reference for exact signatures and statuses.

[SDK guide](../../docs/libraries/sdk.md) · [API reference](../../docs/reference/sdk-log.md) · [Source](src/log.rinzu)
