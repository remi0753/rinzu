# rinzu.testing

Use `assertTrue(condition, message)` and `assertEqual<T: Equality>(actual, expected, message)` to check results. A failed assertion writes failure information and throws `AssertionFailed`.

In a file test's Main Domain, use `runCase(name, body)` for a synchronous case and `await runAsyncCase(name, body)` for an asynchronous case. Each records ordinary Errors, including cleanup Errors, and lets the file continue to its next case. Invalid or duplicate names, overlapping cases, and calls outside a file test's Main Domain raise `InvalidTestCase`. Use `finally` to release resources in the case that acquired them.

Place `.test.rinzu` files under `tests/` beside your package manifest. From the package root, run `rinzu test ./tests`, or specify a test file with `--case NAME` to run one named case. Use the assertion helpers in explicit documentation examples as well.

[Testing guide](../../docs/tooling/testing.md) · [API reference](../../docs/reference/sdk-testing.md) · [Source](src/testing.rinzu)
