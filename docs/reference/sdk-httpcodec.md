# rinzu.httpcodec API reference

Generated from checked public exports and captured source comments.

Language: `rinzu-0.5` · target: `aarch64-apple-darwin` · semantics: `rinzu-language-0.5-r76`.

## rinzu.httpcodec.InvalidRequestLine

error · [source](../../sdk/httpcodec/src/httpcodec.rinzu#L10).

```rinzu
error InvalidRequestLine
```

### field payload

[source](../../sdk/httpcodec/src/httpcodec.rinzu#L11).

```rinzu
payload: Int
```

## rinzu.httpcodec.RequestLine

class · [source](../../sdk/httpcodec/src/httpcodec.rinzu#L5).

```rinzu
class RequestLine
```

### field method

[source](../../sdk/httpcodec/src/httpcodec.rinzu#L6).

```rinzu
const method: String
```

### field target

[source](../../sdk/httpcodec/src/httpcodec.rinzu#L7).

```rinzu
const target: String
```

## rinzu.httpcodec.requestLine

Checked type: `fn(ByteArray)->RequestLine`.

function · [source](../../sdk/httpcodec/src/httpcodec.rinzu#L34).

```rinzu
fn requestLine(data: ByteArray) -> RequestLine
```
