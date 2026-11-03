# Strings and bytes

String is immutable, valid UTF-8 text. It can contain NUL. String indexing counts Unicode scalar values, not encoded bytes or user-perceived grapheme clusters. No implicit Unicode normalization is performed.

```rinzu-test
fn main() {
    try {
        let text = "Hello, 世界";
        let binary = utf8Encode(text);
        if utf8Decode(binary) != text || stringLength("é") != 1 {
            panic("invalid text conversion");
        }
        let answer = parseInt("42");
        if formatInt(answer) != "42" {
            panic("invalid number");
        }
    }
    catch problems {
        panic("text operation failed");
    }
}
```

## Literal forms

Ordinary strings support escapes such as `\n`, `\r`, `\t`, `\"`, `\\`, and `\u{1f642}`. Formatted strings use `f"answer={answer}"`; escape literal braces with `\{` and `\}`. Values use the same display rules as standard output.

Raw strings begin with `r`, zero to sixteen `#` characters, and a quote, then end with the matching delimiter. For example, `r#"C:\data\file"#`. Raw strings preserve line breaks and indentation and do not interpret escapes or interpolation.

## Text APIs

| API | Result and indexing |
| --- | --- |
| `stringLength(text)` | Int scalar count |
| `utf8ByteLength(text)` | CSize encoded byte count |
| `stringGet(text,index)` | Option of a one-scalar String |
| `stringSlice(text,start,end)` | Option of the half-open scalar slice |
| `stringFind(text,needle,start)` | Option of the first scalar index |
| `stringSplit(text,separator)` | List of literal, non-overlapping parts |
| `parseInt(text)` / `formatInt(number)` | Strict full-input decimal parsing / formatting |
| `utf8Encode(text)` / `utf8Decode(bytes)` | Explicit binary conversion; invalid UTF-8 throws Error |

Parsing does not trim whitespace or accept trailing text. Repeated scalar lookups may rescan UTF-8 and copy text. Locale operations, regex, grapheme traversal, and parseFloat are unavailable.

## ByteArray and Bytes(N)

`Bytes(N)` has a compile-time capacity and can include zero padding. `ByteArray` stores an actual binary length and preserves arbitrary bytes, including invalid UTF-8.

```rinzu-test
fn main() {
    let data = byteArray(b"\x00\xffA");
    let pieces: List<ByteArray> = [data, byteArray(b"B")];
    let combined = bytesConcatAll(pieces);
    if bytesLength(combined) != 4 {
        panic("invalid binary length");
    }
    switch bytesGet(combined, 1) {
        case Option.Some(value): {
            if value != CUInt8(255) {
                panic("binary changed");
            }
        }
        case Option.None: {
            panic("missing byte");
        }
    }
}
```

Construct a ByteArray with `byteArray(Bytes(N))` or `byteArray(List<CUInt8>)`. Use `bytesLength`, `bytesGet`, `bytesSlice`, `bytesConcat`, `bytesConcatAll`, `bytesCopy`, and `bytesXor`. Slice indices are half-open Int byte indices; invalid indices return None. `byteLength` returns a CSize length for native calls.

`bytesConcatAll` concatenates in list order and avoids repeatedly copying an ever-growing prefix. Binary scanning helpers are listed in [Core APIs](../libraries/core.md). Worker transfer copies binary data; native calls follow their declared lifetime contracts.

Next: [Errors and recovery](errors.md).
