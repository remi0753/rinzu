# rinzu.shutdown

Bundled cooperative signal and clock support. enableSignals and disableSignals explicitly register and remove the application's signal handling. signalRequested reports a request; clockMillis provides a monotonic clock.

A signal-aware Main polls requests, stops intake, requests Root shutdown, and closes resources in finally. Registration does not make arbitrary native calls interruptible. Always disable a successful registration during cleanup. The default interrupt/force-exit behavior is separate from opting into cooperative handling.

The cooperative development runner checks private registration identity before signaling a child and waits for cleanup acknowledgment. Ordinary use does not create the runner's private receipt files.

[Async and shutdown](../../docs/language/async.md) · [Development restart](../../docs/tooling/editor.md) · [API reference](../../docs/reference/sdk-shutdown.md)
