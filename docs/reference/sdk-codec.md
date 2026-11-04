# rinzu.codec API reference

Generated from checked public exports and captured source comments.

Language: `rinzu-0.5` · target: `aarch64-apple-darwin` · semantics: `rinzu-language-0.5-r78`.

[Machine-readable API v4 snapshot](../../sdk/codec/build/package-api-v4.json).

## rinzu.codec.Document

class · [source](../../sdk/codec/src/codec.rinzu#L7).

```rinzu
class Document interface Encodable
```

Text and a selection measured in Unicode scalar values, not UTF-8 bytes.

### field text

[source](../../sdk/codec/src/codec.rinzu#L9).

```rinzu
const text: String
```

UTF-8 text preserves embedded NUL and all valid Unicode scalars.

### field selection

[source](../../sdk/codec/src/codec.rinzu#L11).

```rinzu
const selection: Int
```

Position in the inclusive range 0..stringLength(text).

## rinzu.codec.Document.encode

Checked type: `fn(Document)->ByteArray`.

Error contract: `throws Error` (declared).

method · [source](../../sdk/codec/src/codec.rinzu#L13).

```rinzu
fn encode(self: Document) -> ByteArray throws Error
```

## rinzu.codec.Encodable

interface · [source](../../sdk/codec/src/codec.rinzu#L2).

```rinzu
interface Encodable
```

An immutable binary representation. Encoding may reject invalid field values.

### method encode

[source](../../sdk/codec/src/codec.rinzu#L3).

```rinzu
fn encode(self: Self) -> ByteArray throws Error;
```

## rinzu.codec.Message

class · [source](../../sdk/codec/src/codec.rinzu#L44).

```rinzu
class Message interface Encodable
```

### field sequence

[source](../../sdk/codec/src/codec.rinzu#L45).

```rinzu
const sequence: Int
```

### field operation

[source](../../sdk/codec/src/codec.rinzu#L46).

```rinzu
const operation: String
```

### field payload

[source](../../sdk/codec/src/codec.rinzu#L47).

```rinzu
const payload: ByteArray
```

## rinzu.codec.Message.encode

Checked type: `fn(Message)->ByteArray`.

Error contract: `throws Error` (declared).

method · [source](../../sdk/codec/src/codec.rinzu#L49).

```rinzu
fn encode(self: Message) -> ByteArray throws Error
```

## rinzu.codec.Settings

class · [source](../../sdk/codec/src/codec.rinzu#L26).

```rinzu
class Settings interface Encodable
```

### field fontSize

[source](../../sdk/codec/src/codec.rinzu#L27).

```rinzu
const fontSize: Int
```

### field tabWidth

[source](../../sdk/codec/src/codec.rinzu#L28).

```rinzu
const tabWidth: Int
```

### field wrap

[source](../../sdk/codec/src/codec.rinzu#L29).

```rinzu
const wrap: Bool
```

## rinzu.codec.Settings.encode

Checked type: `fn(Settings)->ByteArray`.

Error contract: `throws Error` (declared).

method · [source](../../sdk/codec/src/codec.rinzu#L31).

```rinzu
fn encode(self: Settings) -> ByteArray throws Error
```

## rinzu.codec.decodeDocument

Checked type: `fn(ByteArray)->Document`.

Error contract: `throws Error` (declared).

function · [source](../../sdk/codec/src/codec.rinzu#L167).

```rinzu
fn decodeDocument(data: ByteArray) -> Document throws Error
```

## rinzu.codec.decodeMessage

Checked type: `fn(ByteArray)->Message`.

Error contract: `throws Error` (declared).

function · [source](../../sdk/codec/src/codec.rinzu#L209).

```rinzu
fn decodeMessage(data: ByteArray) -> Message throws Error
```

## rinzu.codec.decodeSettings

Checked type: `fn(ByteArray)->Settings`.

Error contract: `throws Error` (declared).

function · [source](../../sdk/codec/src/codec.rinzu#L194).

```rinzu
fn decodeSettings(data: ByteArray) -> Settings throws Error
```

## rinzu.codec.encode

Checked type: `fn(T)->ByteArray throws Error`.

Error contract: `throws Error` (declared).

function · [source](../../sdk/codec/src/codec.rinzu#L235).

```rinzu
fn encode<T interface Encodable>(value: T) -> ByteArray throws Error
```

Encode any value with an explicit Encodable implementation.

## rinzu.codec.encodeDocument

Checked type: `fn(Document)->ByteArray`.

Error contract: `throws Error` (declared).

function · [source](../../sdk/codec/src/codec.rinzu#L163).

```rinzu
fn encodeDocument(value: Document) -> ByteArray throws Error
```

Encode version 1 document data, with at most 1 MiB of UTF-8 text.
Invalid selections throw Error 503; oversized text throws Error 502.
The result owns immutable bytes and requires no native resource cleanup.

```rinzu-test
import rinzu.codec;

fn main() {
    let original = rinzu.codec.Document(text: "日本語\u{0}🙂", selection: 4);
    try {
        let data = rinzu.codec.encode(original);
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

Error contract: `throws Error` (declared).

function · [source](../../sdk/codec/src/codec.rinzu#L205).

```rinzu
fn encodeMessage(value: Message) -> ByteArray throws Error
```

## rinzu.codec.encodeSettings

Checked type: `fn(Settings)->ByteArray`.

Error contract: `throws Error` (declared).

function · [source](../../sdk/codec/src/codec.rinzu#L190).

```rinzu
fn encodeSettings(value: Settings) -> ByteArray throws Error
```
