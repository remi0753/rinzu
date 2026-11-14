# rinzu.httpcodec API reference

Generated from checked public exports and captured source comments.

Language: `rinzu-0.5` · target: `aarch64-apple-darwin` · semantics: `rinzu-language-0.5-r85`.

## rinzu.httpcodec.Decoder

class · [source](../../sdk/httpcodec/src/httpcodec.rinzu#L150).

```rinzu
class Decoder
```

## rinzu.httpcodec.Decoder.close

Checked type: `fn(Decoder)->Unit`.

Error contract: `nothrow` (inferred).

method · [source](../../sdk/httpcodec/src/httpcodec.rinzu#L168).

```rinzu
fn close(self: Decoder)
```

## rinzu.httpcodec.Decoder.create

Checked type: `fn()->Decoder`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/httpcodec/src/httpcodec.rinzu#L156).

```rinzu
fn create() -> Decoder
```

## rinzu.httpcodec.Decoder.end

Checked type: `fn(Decoder)->Unit`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/httpcodec/src/httpcodec.rinzu#L194).

```rinzu
fn end(self: Decoder)
```

## rinzu.httpcodec.Decoder.feed

Checked type: `fn(Decoder,ByteArray,Int)->Option<RequestFrame>`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/httpcodec/src/httpcodec.rinzu#L182).

```rinzu
fn feed(self: Decoder, data: ByteArray, start: Int) -> Option<RequestFrame>
```

## rinzu.httpcodec.Decoder.feedRead

Checked type: `fn(Decoder,rinzu.socket::ReadView,Int)->Option<RequestFrame>`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/httpcodec/src/httpcodec.rinzu#L188).

```rinzu
fn feedRead(self: Decoder, view: socket.ReadView, start: Int) -> Option<RequestFrame>
```

## rinzu.httpcodec.Decoder.withHeadCache

Checked type: `fn(Int)->Decoder`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/httpcodec/src/httpcodec.rinzu#L160).

```rinzu
fn withHeadCache(bytes: Int) -> Decoder
```

## rinzu.httpcodec.Header

class · [source](../../sdk/httpcodec/src/httpcodec.rinzu#L92).

```rinzu
class Header
```

### field name

[source](../../sdk/httpcodec/src/httpcodec.rinzu#L93).

```rinzu
const name: String
```

### field value

[source](../../sdk/httpcodec/src/httpcodec.rinzu#L94).

```rinzu
const value: String
```

## rinzu.httpcodec.InvalidRequest

error · [source](../../sdk/httpcodec/src/httpcodec.rinzu#L117).

```rinzu
error InvalidRequest
```

### field payload

[source](../../sdk/httpcodec/src/httpcodec.rinzu#L118).

```rinzu
payload: Int
```

## rinzu.httpcodec.InvalidRequestLine

error · [source](../../sdk/httpcodec/src/httpcodec.rinzu#L60).

```rinzu
error InvalidRequestLine
```

### field payload

[source](../../sdk/httpcodec/src/httpcodec.rinzu#L61).

```rinzu
payload: Int
```

## rinzu.httpcodec.RequestFrame

class · [source](../../sdk/httpcodec/src/httpcodec.rinzu#L105).

```rinzu
class RequestFrame
```

### field end

[source](../../sdk/httpcodec/src/httpcodec.rinzu#L106).

```rinzu
const end: Int
```

### field head

[source](../../sdk/httpcodec/src/httpcodec.rinzu#L107).

```rinzu
const head: RequestHead
```

### field body

[source](../../sdk/httpcodec/src/httpcodec.rinzu#L108).

```rinzu
const body: ByteArray
```

## rinzu.httpcodec.RequestHead

class · [source](../../sdk/httpcodec/src/httpcodec.rinzu#L97).

```rinzu
class RequestHead
```

### field method

[source](../../sdk/httpcodec/src/httpcodec.rinzu#L98).

```rinzu
const method: String
```

### field target

[source](../../sdk/httpcodec/src/httpcodec.rinzu#L99).

```rinzu
const target: String
```

### field headers

[source](../../sdk/httpcodec/src/httpcodec.rinzu#L100).

```rinzu
const headers: List<Header>
```

### field closing

[source](../../sdk/httpcodec/src/httpcodec.rinzu#L101).

```rinzu
const closing: Bool
```

### field version

[source](../../sdk/httpcodec/src/httpcodec.rinzu#L102).

```rinzu
const version: Int
```

## rinzu.httpcodec.RequestLine

class · [source](../../sdk/httpcodec/src/httpcodec.rinzu#L55).

```rinzu
class RequestLine
```

### field method

[source](../../sdk/httpcodec/src/httpcodec.rinzu#L56).

```rinzu
const method: String
```

### field target

[source](../../sdk/httpcodec/src/httpcodec.rinzu#L57).

```rinzu
const target: String
```

## rinzu.httpcodec.requestLine

Checked type: `fn(ByteArray)->RequestLine`.

Error contract: `throws(InvalidRequestLine)` (inferred).

function · [source](../../sdk/httpcodec/src/httpcodec.rinzu#L84).

```rinzu
fn requestLine(data: ByteArray) -> RequestLine
```
