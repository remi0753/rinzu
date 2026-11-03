# rinzu.testing

Bundled assertion helpers: assertTrue(condition,message) and assertEqual<T: Equality>(actual,expected,message). Failure prints useful diagnostics and throws AssertionFailed. Let the failure reach a test boundary; put fixture/Owner cleanup in finally. Assertions do not close resources.

Use rinzu test PACKAGE --unit for private zero-argument test_* functions, or --doc FILE for explicit rinzu-test blocks. Each selected case runs in an isolated worker/scratch environment. Plain test runs the package main.

[Testing guide](../../docs/tooling/testing.md) · [API reference](../../docs/reference/sdk-testing.md) · [Source](src/testing.rinzu)
