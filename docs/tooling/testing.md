# Testing and documentation tests

Testing uses a directory package with a manifest. Plain `rinzu test ./app` runs that package's main. Use `--unit` to discover isolated test cases from ordinary source.

## Write a unit test

Add a private, zero-argument function named `test_*` to package source. Its result must be Unit; it can be synchronous or async.

```rinzu
import rinzu.testing;

fn add(left: Int, right: Int) -> Int {
    return left + right;
}

fn test_sum() {
    rinzu.testing.assertEqual<Int>(add(20, 22), 42, "sum");
}

async fn test_async_value() {
    rinzu.testing.assertTrue(true, "async test");
}

fn main() {}
```

```sh
rinzu test ./app --unit --list
rinzu test ./app --unit
rinzu test ./app --unit --filter test_sum
rinzu test ./app --unit --format json --timeout-ms 5000
```

The selected package is checked before filtering, so errors in an unselected function can still fail the command. Unit mode does not execute the original main or dependency SDK tests.

`assertTrue(condition,message)` and `assertEqual<T: Equality>(actual,expected,message)` throw AssertionFailed and print useful failure information. Let test failures reach the test boundary. Use finally for resource cleanup. Native handles and Error values need explicit state/counter checks when they lack Equality.

## Execute documentation examples

A fenced block labeled `rinzu-test` is a complete program with main:

```rinzu-test
import rinzu.testing;

fn main() {
    try {
        rinzu.testing.assertEqual<Int>(20 + 22, 42, "documented answer");
    }
    catch problems {
        panic("documentation assertion failed");
    }
}
```

Run the example in this page from the compiler checkout:

```sh
rinzu test sdk/testing --doc "$PWD/docs/tooling/testing.md" --format json
```

Document paths resolve relative to the selected package, not the shell directory. The absolute path above selects this repository guide. For another project, pass its package and the desired document. A document is an explicit input; plain `rinzu` fences are not executable cases. Generated API Markdown can preserve executable examples from adjacent source doc comments.

## Isolate resources

Each case runs in a separate worker process with scratch space and a dedicated cwd. Use that scratch environment for file fixtures, including `RINZU_TEST_TMP` when selecting paths. Do not share databases, native windows, or fixed external ports between cases.

Timeout and failure reporting do not make arbitrary native calls interruptible. Follow each SDK's physical cleanup contract. Make random/property-style tests deterministic by supplying a seed and reporting it with failures; the language does not provide an automatic shrinking framework.

## Check JIT and AOT behavior

```sh
rinzu verify-aot ./app
```

This compares supported execution semantics, settlement, and complete captured stdout/stderr bytes. Keep nondeterministic timestamps, remote responses, and process-specific values out of output intended for exact comparison. It is separate from unit discovery and is not a benchmark.
