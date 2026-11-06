# JSON

Import `rinzu.json` in a standalone file or package. `parse` accepts a UTF-8 `ByteArray` and `parseString` accepts a `String`. Both validate one complete JSON value and return a `Value` that shares its input and index with any extracted subtree.

```rinzu-test
import rinzu.json;

type Transaction = {
    id: Int,
    amount: Int,
    note: Option<String>
};

fn main() {
    try {
        let transaction: Transaction = rinzu.json.parseStringWithTy(r#"{"id":9007199254740993,"amount":1200}"#);
        if transaction.id != 9007199254740993 || transaction.amount != 1200
            || transaction.note != Option<String>.None() {
            panic("unexpected transaction");
        }
    }
    catch problems {
        panic("invalid transaction");
    }
}
```

## Inspect and build values

`kind` returns `Kind.Null`, `Boolean`, `Number`, `Text`, `Array`, or `Object`. Values belong to their own Domain. To parse in a Worker, pass the input bytes and return the transfer-safe result fields.

| Operation | Result |
| --- | --- |
| `field(object, key)` | `Option<Value>`. An existing key with a JSON null returns `Some`. |
| `at(array, index)` | `Option<Value>`. The index is zero-based. Out-of-range indices return `None`. |
| `arrayLength(array)`, `objectLength(object)` | Element or entry count. |
| `entryAt(object, index)` | `Option<{ key: String, value: Value }>` in input or insertion order. |
| `asBool(value)`, `asString(value)` | The Boolean or decoded String. |
| `numberText(value)` | The original number lexeme or the rendering of a constructed number. |
| `asInt(value)` | An integer lexeme in the signed 64-bit range. |
| `asFloat(value)` | The nearest finite binary64 value, with ties rounded to even and the sign of zero preserved. |
| `intField(object, key)`, `floatField(object, key)` | A numeric lookup that returns `None` for a missing key. |
| `nullValue()`, `boolean(value)`, `text(value)`, `intValue(value)` | Construct scalar JSON. |
| `number(token)`, `floatValue(value)` | Construct JSON from a valid number lexeme or a finite Float. |
| `array(values)`, `object(fields)` | Wrap an immutable `List<Value>` or `Map<String, Value>`. |
| `encode(value)`, `stringify(value)` | Produce a UTF-8 `ByteArray` or a `String`. |

Accessors throw `TypeMismatch` when the value has the wrong kind. `NumberError` distinguishes invalid syntax, expected integer, out of range, and non-finite input. Integer access treats `1.0` and `1e0` as non-integer lexemes. Float constructors use shortest round-trip digits, decimal notation for exponents −6 through 20, and an explicit exponent sign otherwise.

Output preserves the original bytes of parsed subtrees and includes the surrounding whitespace when encoding a parsed root. Constructed values use compact separators and preserve insertion order.

## Decode directly to a type

Use `parseWithTy<T>` for bytes and `parseStringWithTy<T>` for text. The target can also be inferred from the surrounding type. Supported types are `Bool`, `Int`, `Float`, `String`, structural records, `List<T>`, `Map<String, T>`, and `Option<T>`. Record field order is independent of input order. An optional field can be omitted or set to JSON null. Unknown fields and missing required fields raise `SchemaMismatch`.

The schema error payload includes a byte offset and a JSON Pointer path. For example, `/0/amount` addresses the first array element's amount. Unknown and missing fields also carry the decoded key.

## Limits and diagnostics

`defaultParseLimits` and `defaultEncodeLimits` allow 128 MiB of UTF-8, 1,024 container levels, and 16,777,216 nodes. Each object key counts as a node. Limits are inclusive. A scalar root has depth zero.

Use `parseWithLimits`, `parseStringWithLimits`, `parseWithTyAndLimits`, or `parseStringWithTyAndLimits` with a `ParseLimits` record. Use `encodeWithLimits` or `stringifyWithLimits` with an `EncodeLimits` record. The fields are `maxInputBytes` or `maxOutputBytes`, `maxDepth`, and `maxNodes`. Valid ranges are 1–268,435,456 bytes, 0–4,096 levels, and 1–33,554,432 nodes.

`DecodeFailed` reports a reason and a zero-based byte offset. Object keys are compared after decoding escapes. For a duplicate, the offset identifies the repeated key's opening quote and `firstOffset` identifies its first occurrence. `LimitExceeded` identifies the exceeded limit and includes the input byte offset for a parse. `InvalidLimits` identifies an invalid setting before scanning.

The transaction report fixture reads a file selected through `rinzu.process.arguments()`, closes it in `finally`, and prints a count and a checked sum of integer amounts. [Report source](../../tests/fixtures/programs/json-report/src/main.rinzu)
