# rinzu.codec API reference

Generated from checked public exports and captured source comments.

Language: `rinzu-0.5` · target: `aarch64-apple-darwin` · semantics: `rinzu-language-0.5-r76`.

## rinzu.codec.Document

class · [source](../../sdk/codec/src/codec.rinzu#L2).

```rinzu
class Document
```

Text and a selection measured in Unicode scalar values, not UTF-8 bytes.

### field text

[source](../../sdk/codec/src/codec.rinzu#L4).

```rinzu
const text: String
```

UTF-8 text preserves embedded NUL and all valid Unicode scalars.

### field selection

[source](../../sdk/codec/src/codec.rinzu#L6).

```rinzu
const selection: Int
```

Position in the inclusive range 0..stringLength(text).

## rinzu.codec.Message

class · [source](../../sdk/codec/src/codec.rinzu#L15).

```rinzu
class Message
```

### field sequence

[source](../../sdk/codec/src/codec.rinzu#L16).

```rinzu
const sequence: Int
```

### field operation

[source](../../sdk/codec/src/codec.rinzu#L17).

```rinzu
const operation: String
```

### field payload

[source](../../sdk/codec/src/codec.rinzu#L18).

```rinzu
const payload: ByteArray
```

## rinzu.codec.Settings

class · [source](../../sdk/codec/src/codec.rinzu#L9).

```rinzu
class Settings
```

### field fontSize

[source](../../sdk/codec/src/codec.rinzu#L10).

```rinzu
const fontSize: Int
```

### field tabWidth

[source](../../sdk/codec/src/codec.rinzu#L11).

```rinzu
const tabWidth: Int
```

### field wrap

[source](../../sdk/codec/src/codec.rinzu#L12).

```rinzu
const wrap: Bool
```

## rinzu.codec.decodeDocument

Checked type: `fn(ByteArray)->Document`.

function · [source](../../sdk/codec/src/codec.rinzu#L123).

```rinzu
fn decodeDocument(data: ByteArray) -> Document
```

## rinzu.codec.decodeMessage

Checked type: `fn(ByteArray)->Message`.

function · [source](../../sdk/codec/src/codec.rinzu#L192).

```rinzu
fn decodeMessage(data: ByteArray) -> Message
```

## rinzu.codec.decodeSettings

Checked type: `fn(ByteArray)->Settings`.

function · [source](../../sdk/codec/src/codec.rinzu#L158).

```rinzu
fn decodeSettings(data: ByteArray) -> Settings
```

## rinzu.codec.encodeDocument

Checked type: `fn(Document)->ByteArray`.

function · [source](../../sdk/codec/src/codec.rinzu#L111).

```rinzu
fn encodeDocument(value: Document) -> ByteArray
```

Encode version 1 document data, with at most 1 MiB of UTF-8 text.
Invalid selections throw Error 503; oversized text throws Error 502.
The result owns immutable bytes and requires no native resource cleanup.

```rinzu-test
import rinzu.codec;

fn main() {
    let original = rinzu.codec.Document(text: "日本語\u{0}🙂", selection: 4);
    try {
        let data = rinzu.codec.encodeDocument(original);
        let decoded = rinzu.codec.decodeDocument(data);
        if decoded.text != original.text || decoded.selection != original.selection {
            panic("failed");
        }
    }
    catch errors {
        panic("failed");
    }
}
```

## rinzu.codec.encodeMessage

Checked type: `fn(Message)->ByteArray`.

function · [source](../../sdk/codec/src/codec.rinzu#L169).

```rinzu
fn encodeMessage(value: Message) -> ByteArray
```

## rinzu.codec.encodeSettings

Checked type: `fn(Settings)->ByteArray`.

function · [source](../../sdk/codec/src/codec.rinzu#L146).

```rinzu
fn encodeSettings(value: Settings) -> ByteArray
```
