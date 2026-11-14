# rinzu.shutdown API reference

Generated from checked public exports and captured source comments.

Language: `rinzu-0.5` · target: `aarch64-apple-darwin` · semantics: `rinzu-language-0.5-r85`.

## rinzu.shutdown.clockMillis

Checked type: `native()->Int`.

native-function · [source](../../sdk/shutdown/src/shutdown.rinzu#L10).

```rinzu
fn clockMillis() -> Int symbol "rinzu_shutdown_clock";
```

## rinzu.shutdown.disableSignals

Checked type: `fn()->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/shutdown/src/shutdown.rinzu#L17).

```rinzu
fn disableSignals() -> Int
```

## rinzu.shutdown.enableSignals

Checked type: `fn()->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/shutdown/src/shutdown.rinzu#L13).

```rinzu
fn enableSignals() -> Int
```

## rinzu.shutdown.signalRequested

Checked type: `native()->CInt32`.

native-function · [source](../../sdk/shutdown/src/shutdown.rinzu#L8).

```rinzu
fn signalRequested() -> CInt symbol "rinzu_shutdown_signal";
```
