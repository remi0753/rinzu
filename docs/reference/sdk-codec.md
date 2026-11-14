# rinzu.codec API reference

Generated from checked public exports and captured source comments.

Language: `rinzu-0.5` · target: `aarch64-apple-darwin` · semantics: `rinzu-language-0.5-r85`.

## rinzu.codec.Document

class · [source](../../sdk/codec/src/codec.rinzu#L11).

```rinzu
class Document interface Encodable
```

Text and a selection measured in Unicode scalar values, not UTF-8 bytes.

### field text

[source](../../sdk/codec/src/codec.rinzu#L13).

```rinzu
const text: String
```

UTF-8 text preserves embedded NUL and all valid Unicode scalars.

### field selection

[source](../../sdk/codec/src/codec.rinzu#L15).

```rinzu
const selection: Int
```

Position in the inclusive range 0..stringLength(text).

## rinzu.codec.Document.encode

Checked type: `fn(Document) throws(Error)->ByteArray`.

Error contract: `throws(Error)` (declared).

method · [source](../../sdk/codec/src/codec.rinzu#L17).

```rinzu
fn encode(self: Document) throws(Error) -> ByteArray
```

## rinzu.codec.Encodable

interface · [source](../../sdk/codec/src/codec.rinzu#L6).

```rinzu
interface Encodable
```

An immutable binary representation. Encoding may reject invalid field values.

### method encode

[source](../../sdk/codec/src/codec.rinzu#L7).

```rinzu
fn encode(self: Self) throws(Error) -> ByteArray;
```

## rinzu.codec.Message

class · [source](../../sdk/codec/src/codec.rinzu#L51).

```rinzu
class Message interface Encodable
```

### field sequence

[source](../../sdk/codec/src/codec.rinzu#L52).

```rinzu
const sequence: Int
```

### field operation

[source](../../sdk/codec/src/codec.rinzu#L53).

```rinzu
const operation: String
```

### field payload

[source](../../sdk/codec/src/codec.rinzu#L54).

```rinzu
const payload: ByteArray
```

## rinzu.codec.Message.encode

Checked type: `fn(Message) throws(Error)->ByteArray`.

Error contract: `throws(Error)` (declared).

method · [source](../../sdk/codec/src/codec.rinzu#L56).

```rinzu
fn encode(self: Message) throws(Error) -> ByteArray
```

## rinzu.codec.Settings

class · [source](../../sdk/codec/src/codec.rinzu#L33).

```rinzu
class Settings interface Encodable
```

### field fontSize

[source](../../sdk/codec/src/codec.rinzu#L34).

```rinzu
const fontSize: Int
```

### field tabWidth

[source](../../sdk/codec/src/codec.rinzu#L35).

```rinzu
const tabWidth: Int
```

### field wrap

[source](../../sdk/codec/src/codec.rinzu#L36).

```rinzu
const wrap: Bool
```

## rinzu.codec.Settings.encode

Checked type: `fn(Settings) throws(Error)->ByteArray`.

Error contract: `throws(Error)` (declared).

method · [source](../../sdk/codec/src/codec.rinzu#L38).

```rinzu
fn encode(self: Settings) throws(Error) -> ByteArray
```

## rinzu.codec.decodeDocument

Checked type: `fn(ByteArray) throws(Error)->Document`.

Error contract: `throws(Error)` (declared).

function · [source](../../sdk/codec/src/codec.rinzu#L172).

```rinzu
fn decodeDocument(data: ByteArray) throws(Error) -> Document
```

## rinzu.codec.decodeMessage

Checked type: `fn(ByteArray) throws(Error)->Message`.

Error contract: `throws(Error)` (declared).

function · [source](../../sdk/codec/src/codec.rinzu#L221).

```rinzu
fn decodeMessage(data: ByteArray) throws(Error) -> Message
```

## rinzu.codec.decodeSettings

Checked type: `fn(ByteArray) throws(Error)->Settings`.

Error contract: `throws(Error)` (declared).

function · [source](../../sdk/codec/src/codec.rinzu#L206).

```rinzu
fn decodeSettings(data: ByteArray) throws(Error) -> Settings
```

## rinzu.codec.encode

Checked type: `fn(T) throws(Error)->ByteArray`.

Error contract: `throws(Error)` (declared).

function · [source](../../sdk/codec/src/codec.rinzu#L254).

```rinzu
fn encode<T interface Encodable>(value: T) throws(Error) -> ByteArray
```

Encode any value with an explicit Encodable implementation.

## rinzu.codec.encodeDocument

Checked type: `fn(Document) throws(Error)->ByteArray`.

Error contract: `throws(Error)` (declared).

function · [source](../../sdk/codec/src/codec.rinzu#L168).

```rinzu
fn encodeDocument(value: Document) throws(Error) -> ByteArray
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

Checked type: `fn(Message) throws(Error)->ByteArray`.

Error contract: `throws(Error)` (declared).

function · [source](../../sdk/codec/src/codec.rinzu#L217).

```rinzu
fn encodeMessage(value: Message) throws(Error) -> ByteArray
```

## rinzu.codec.encodeSettings

Checked type: `fn(Settings) throws(Error)->ByteArray`.

Error contract: `throws(Error)` (declared).

function · [source](../../sdk/codec/src/codec.rinzu#L202).

```rinzu
fn encodeSettings(value: Settings) throws(Error) -> ByteArray
```
