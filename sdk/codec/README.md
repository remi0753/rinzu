# rinzu.codec

Additional pure source SDK for versioned Document, Settings, and Message binary formats. It uses const classes and ordinary Errors; no native dependency is required.

Public operations are encodeDocument/decodeDocument, encodeSettings/decodeSettings, and encodeMessage/decodeMessage. Encoding returns ByteArray; decoding returns the corresponding value. Formats use strict framing, UTF-8, full-length, and range checks. Document data is bounded to 1 MiB and messages to 64 KiB. This is not a general recursive serializer or JSON API.

Document stores text and a Unicode-scalar selection. Settings stores fontSize/tabWidth/wrap. Message stores sequence/operation/binary payload. Source doc comments include executable examples.

[SDK guide](../../docs/libraries/sdk.md) · [API reference](../../docs/reference/sdk-codec.md) · [Source](src/codec.rinzu)
