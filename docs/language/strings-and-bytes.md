# Strings and bytes

String is immutable, valid UTF-8 text. It can contain NUL. String indexing counts Unicode scalar values, not encoded bytes or user-perceived grapheme clusters. No implicit Unicode normalization is performed.

```rinzu-test
import rinzu.core;
import rinzu.number;
import rinzu.string;

fn main() {
    try {
        let text = "Hello, 世界";
        let binary = rinzu.string.encodeUtf8(text);
        if rinzu.string.decodeUtf8(binary) != text || rinzu.string.length("é") != 1 {
            rinzu.core.panic("invalid text conversion");
        }
        let answer = rinzu.number.parseInt("42");
        if rinzu.number.formatInt(answer) != "42" {
            rinzu.core.panic("invalid number");
        }
    }
    catch problems {
        rinzu.core.panic("text operation failed");
    }
}
```

## Literal forms

Ordinary strings support escapes such as `\n`, `\r`, `\t`, `\"`, `\\`, and `\u{1f642}`. Formatted strings use `f"answer={answer}"`; escape literal braces with `\{` and `\}`. Values use the same display rules as standard output.

Raw strings begin with `r`, zero to sixteen `#` characters, and a quote, then end with the matching delimiter. For example, `r#"C:\data\file"#`. Raw strings preserve line breaks and indentation and do not interpret escapes or interpolation.

## Text APIs

| API | Result and indexing |
| --- | --- |
| `rinzu.string.length(text)` | Int scalar count |
| `rinzu.string.utf8ByteLength(text)` | CSize encoded byte count |
| `rinzu.string.get(text,index)` | Option of a one-scalar String |
| `rinzu.string.slice(text,start,end)` | Option of the half-open scalar slice |
| `rinzu.string.find(text,needle,start)` | Option of the first scalar index |
| `rinzu.string.split(text,separator)` | List of literal, non-overlapping parts |
| `rinzu.number.parseInt(text)` / `rinzu.number.formatInt(number)` | Strict full-input decimal parsing / formatting |
| `rinzu.string.encodeUtf8(text)` / `rinzu.string.decodeUtf8(bytes)` | Explicit binary conversion. Invalid UTF-8 throws Error. |

Parsing does not trim whitespace or accept trailing text. Repeated scalar lookups may rescan UTF-8 and copy text. Locale operations, regex, grapheme traversal, and parseFloat are unavailable.

## ByteArray and Bytes(N)

`Bytes(N)` has a compile-time capacity and can include zero padding. `ByteArray` stores an actual binary length and preserves arbitrary bytes, including invalid UTF-8.

```rinzu-test
import rinzu.bytes;
import rinzu.core;

fn main() {
    let data = rinzu.bytes.from(b"\x00\xffA");
    let pieces: List<ByteArray> = [data, rinzu.bytes.from(b"B")];
    let combined = rinzu.bytes.concatAll(pieces);
    if rinzu.bytes.length(combined) != 4 {
        rinzu.core.panic("invalid binary length");
    }
    switch rinzu.bytes.get(combined, 1) {
        case Option.Some(value): {
            if value != CUInt8(255) {
                rinzu.core.panic("binary changed");
            }
        }
        case Option.None: {
            rinzu.core.panic("missing byte");
        }
    }
}
```

Construct a ByteArray with `rinzu.bytes.from(Bytes(N))` or `rinzu.bytes.from(List<CUInt8>)`. Use `rinzu.bytes.length`, `rinzu.bytes.get`, `rinzu.bytes.slice`, `rinzu.bytes.concat`, `rinzu.bytes.concatAll`, `rinzu.bytes.copy`, and `rinzu.bytes.xor`. Slice indices are half-open Int byte indices. Invalid indices return None. `rinzu.bytes.byteLength` returns a CSize length for native calls.

`rinzu.bytes.concatAll` concatenates in list order and avoids repeatedly copying an ever-growing prefix. Binary scanning helpers are listed in [Core APIs](../libraries/core.md). Worker transfer copies binary data; native calls follow their declared lifetime contracts.

Next: [Errors and recovery](errors.md).
