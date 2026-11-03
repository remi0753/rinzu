# rinzu.consumption

Additional SDK demonstrating closed synchronous native callback consumers. strings(fn(CSize)->String,CSize) and bytes4(fn(CSize)->Bytes(4),CSize) return CSize checksums. The C consumer reads each returned payload twice; String reads stop at its first NUL and fixed bytes include all four bytes.

Payload/control reclamation depends on the toolchain's proof of the selected closed consumer and actual call/entry/child settlement. Unknown consumers retain the conservative lifetime contract. Stable callback entry addresses remain reserved. This SDK does not provide general long-lived capturing registration or managed-address export.

[Native guide](../../docs/language/native-interop.md) · [API reference](../../docs/reference/sdk-consumption.md) · [Source](src/consumption.rinzu)
