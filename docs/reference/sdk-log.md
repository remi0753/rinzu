# rinzu.log API reference

Generated from checked public exports and captured source comments.

Language: `rinzu-0.5` · target: `aarch64-apple-darwin` · semantics: `rinzu-language-0.5-r78`.

[Machine-readable API v4 snapshot](../../sdk/log/build/package-api-v4.json).

## rinzu.log.Logger

class · [source](../../sdk/log/src/log.rinzu#L3).

```rinzu
class Logger
```

### field file

[source](../../sdk/log/src/log.rinzu#L4).

```rinzu
const file: rinzu.io.File
```

### field busy

[source](../../sdk/log/src/log.rinzu#L5).

```rinzu
busy: Bool
```

### field closed

[source](../../sdk/log/src/log.rinzu#L6).

```rinzu
closed: Bool
```

## rinzu.log.close

Checked type: `async(Logger)->Int`.

Error contract: `throws Error` (declared).

function · [source](../../sdk/log/src/log.rinzu#L64).

```rinzu
async fn close(logger: Logger) -> Int throws Error
```

## rinzu.log.emit

Checked type: `async(Logger,Int,Int,Int,Int)->Int`.

Error contract: `throws Error` (declared).

function · [source](../../sdk/log/src/log.rinzu#L25).

```rinzu
async fn emit(logger: Logger, level: Int, operationId: Int, event: Int, status: Int) -> Int throws Error
```

## rinzu.log.flush

Checked type: `async(Logger)->Int`.

Error contract: `throws Error` (declared).

function · [source](../../sdk/log/src/log.rinzu#L49).

```rinzu
async fn flush(logger: Logger) -> Int throws Error
```

## rinzu.log.open

Checked type: `async(String)->Logger`.

Error contract: `throws Error` (declared).

function · [source](../../sdk/log/src/log.rinzu#L9).

```rinzu
async fn open(path: String) -> Logger throws Error
```
