# rinzu.consumption API reference

Generated from checked public exports and captured source comments.

Language: `rinzu-0.5` · target: `aarch64-apple-darwin` · semantics: `rinzu-language-0.5-r85`.

## rinzu.consumption.bytes4

Checked type: `fn(fn(CUInt64)->Bytes(4),CUInt64)->CUInt64`.

Error contract: `nothrow` (inferred).

function · [source](../../sdk/consumption/src/consumption.rinzu#L11).

```rinzu
fn bytes4(producer: fn (CSize) -> Bytes(4), count: CSize) -> CSize
```

## rinzu.consumption.strings

Checked type: `fn(fn(CUInt64)->String,CUInt64)->CUInt64`.

Error contract: `nothrow` (inferred).

function · [source](../../sdk/consumption/src/consumption.rinzu#L7).

```rinzu
fn strings(producer: fn (CSize) -> String, count: CSize) -> CSize
```
