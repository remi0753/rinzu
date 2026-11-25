# Testing and documentation tests

Place application modules under `src/` and tests under `tests/`, beside the package's `rinzu.toml`. The runner discovers files ending in `.test.rinzu`. Each file has one synchronous or asynchronous `main` with no arguments or type parameters and a `Unit` result. Its imports determine which application modules are compiled for that file. Use a dotted import to access an exported application module and a relative file import to share helpers under `tests/`. Ordinary application package checks, runs, and builds follow the application entry and its `src/` imports.

A test file can call `rinzu.testing.runCase` for a synchronous case and await `rinzu.testing.runAsyncCase` for an asynchronous case. The file's `main` runs once, and each selected case body runs once. An ordinary Error in a case is reported as a failure and the next case can run. Assertions directly in `main` affect the file's result.

## Write and select tests

In an application package under `app/`, add the following to `src/math.rinzu`.

```rinzu
import rinzu.timer;

fn add(left: Int, right: Int) -> Int {
    return left + right;
}

async fn addLater(left: Int, right: Int) -> Int {
    await rinzu.timer.sleep(1);
    return add(left, right);
}

export add;
export addLater;
```

Create `tests/math.test.rinzu` in the same package.

```rinzu
import math;
import rinzu.testing as testing;

async fn main() {
    testing.runCase(
        "positive values",
        fn () {
            testing.assertEqual<Int>(math.add(20, 22), 42, "sum");
        }
    );
    testing.runCase(
        "negative values",
        fn () {
            testing.assertEqual<Int>(math.add(-20, -22), -42, "sum");
        }
    );
    await testing.runAsyncCase(
        "delayed sum",
        async fn () {
            let sum = await math.addLater(20, 22);
            testing.assertEqual<Int>(sum, 42, "delayed sum");
        }
    );
}
```

From the directory containing `app/`, run the suite or select a file or case.

```sh
rinzu test ./app/tests
rinzu test ./app/tests --list
rinzu test ./app/tests --filter math.test.rinzu --format json
rinzu test ./app/tests/math.test.rinzu --case 'negative values'
```

File-only `--list` reports the selected paths. `--filter` matches a substring of the file ID, such as `file:tests/math.test.rinzu`, before compiling the selected files. Directories under `tests/` can be selected separately. Nested packages have their own test roots.

Use `--case` for a file-only execution with one explicit test file. It matches the exact case name and reports a failure if `main` completes without finding it. Use `--case=NAME` for names starting with a hyphen. The selected file still compiles and runs its entire `main`, including code outside case bodies. Put case-specific setup and cleanup inside the case body.

Run cases sequentially in the test file's Main Domain. Give each case a unique name containing at least one non-whitespace character, with at most 4,096 UTF-8 bytes and no NUL character. The case APIs raise `InvalidTestCase` for invalid names, overlapping cases, and calls outside a file test's Main Domain. The runner records misuse detected inside a file test even if the test catches that Error.

`assertTrue(condition, message)` and `assertEqual<T: Equality>(actual, expected, message)` throw `AssertionFailed` and display failure information. Use `finally` to release resources and let case failures reach the case or file boundary. Native handles and Error values need explicit state checks when they lack Equality. Plain `rinzu test ./app` runs the package's application `main`.

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

Resolve relative document paths from the selected package root. The absolute path above selects this repository guide from the checkout root. Repeat `--doc` to select several documents. Combine a test path with `--doc` to include both kinds of test in the same report. The document runner executes the `rinzu-test` fences, which can use the assertion helpers. Named case registration belongs in `.test.rinzu` files. Generated API Markdown preserves executable examples from source doc comments.

## Isolate resources

Each test file and documentation example runs in a separate worker process with its scratch directory as the working directory. Named cases within a file share its process and deadline. Use `RINZU_TEST_TMP` to find the scratch directory and clean up resources inside the case that acquired them.

`--timeout-ms` bounds the whole worker run, including compilation and cleanup. The default is 60,000 ms, and values from 1 to 600,000 ms are accepted. A file passes when its `main` and named cases succeed and the runner confirms runtime cleanup.

Timeout and failure reporting do not make arbitrary native calls interruptible. Follow each SDK's physical cleanup contract. Make random/property-style tests deterministic by supplying a seed and reporting it with failures; the language does not provide an automatic shrinking framework.

## Read test results

Use `--format json` for the `rinzu.test-report` schema. A file-test invocation uses version 2. Its `entries` array contains the selected files and any explicit document blocks. Each file entry has its own `cases` array. `summary` counts files and document blocks, while `caseSummary` counts named cases that started. Document-only invocations use version 1 with the document blocks in the top-level `cases` array.

File and documentation tests return 0 for a successful listing or run, and 1 for a failed run or when nothing matches. An interrupted run uses the signal-based exit code.

## Check JIT and AOT behavior

```sh
rinzu verify-aot ./app
```

This compares supported execution semantics, settlement, and complete captured stdout/stderr bytes for the selected program. Keep nondeterministic timestamps, remote responses, and process-specific values out of output intended for exact comparison.
