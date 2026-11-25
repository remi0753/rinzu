# rinzu.testing API reference

Generated from checked public exports and captured source comments.

Language: `rinzu-0.5` · target: `aarch64-apple-darwin` · semantics: `rinzu-language-0.5-r85`.

## rinzu.testing.AssertionFailed

error · [source](../../sdk/testing/src/testing.rinzu#L15).

```rinzu
error AssertionFailed
```

A recoverable test assertion failure, including the caller's message.

## rinzu.testing.InvalidTestCase

error · [source](../../sdk/testing/src/testing.rinzu#L18).

```rinzu
error InvalidTestCase
```

A named case could not start in the current test context.

## rinzu.testing.assertEqual

Checked type: `fn(T,T,String)->Unit`.

Error contract: inferred for each specialization.

function · [source](../../sdk/testing/src/testing.rinzu#L98).

```rinzu
fn assertEqual<T: Equality>(actual: T, expected: T, message: String)
```

Compare actual and expected with the normal Equality marker rules.
A mismatch prints the message and both values, then throws AssertionFailed.
Observe native Owner handles separately; this helper does not close resources.

```rinzu-test
import rinzu.testing;

fn main() {
    rinzu.testing.assertEqual<ByteArray>(byteArray(b"\x00\xff"), byteArray(b"\x00\xff"), "binary equality");
}
```

## rinzu.testing.assertTrue

Checked type: `fn(Bool,String)->Unit`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/testing/src/testing.rinzu#L80).

```rinzu
fn assertTrue(condition: Bool, message: String)
```

Assert a condition using AssertionFailed on failure.
Success returns Unit without output. A failed assertion prints the message.
Let failures propagate from tests; use finally for fixture/Owner cleanup.

```rinzu-test
import rinzu.testing;

fn main() {
    rinzu.testing.assertTrue(true, "positive assertion");
}
```

## rinzu.testing.runAsyncCase

Checked type: `async(String,fn() throws(Error)->Future<Unit>) throws(InvalidTestCase)->Unit`.

Error contract: `throws(InvalidTestCase)` (declared).

function · [source](../../sdk/testing/src/testing.rinzu#L55).

```rinzu
async fn runAsyncCase(name: String, body: fn () throws(Error) -> Future<Unit>) throws(InvalidTestCase)
```

Run and await a named asynchronous case in a .test.rinzu file.

## rinzu.testing.runCase

Checked type: `fn(String,fn() throws(Error)->Unit) throws(InvalidTestCase)->Unit`.

Error contract: `throws(InvalidTestCase)` (declared).

function · [source](../../sdk/testing/src/testing.rinzu#L40).

```rinzu
fn runCase(name: String, body: fn () throws(Error) -> Unit) throws(InvalidTestCase)
```

Run a named synchronous case in a .test.rinzu file.
Record ordinary Errors as case failures and return to the file's main.
