# rinzu.log API reference

Generated from checked public exports and captured source comments.

Language: `rinzu-0.5` · target: `aarch64-apple-darwin` · semantics: `rinzu-language-0.5-r85`.

## rinzu.log.Logger

class · [source](../../sdk/log/src/log.rinzu#L6).

```rinzu
class Logger
```

### field file

[source](../../sdk/log/src/log.rinzu#L7).

```rinzu
const file: rinzu.io.File
```

### field busy

[source](../../sdk/log/src/log.rinzu#L8).

```rinzu
busy: Bool
```

### field closed

[source](../../sdk/log/src/log.rinzu#L9).

```rinzu
closed: Bool
```

## rinzu.log.close

Checked type: `async(Logger) throws(Error)->Int`.

Error contract: `throws(Error)` (declared).

function · [source](../../sdk/log/src/log.rinzu#L67).

```rinzu
async fn close(logger: Logger) throws(Error) -> Int
```

## rinzu.log.emit

Checked type: `async(Logger,Int,Int,Int,Int) throws(Error)->Int`.

Error contract: `throws(Error)` (declared).

function · [source](../../sdk/log/src/log.rinzu#L28).

```rinzu
async fn emit(logger: Logger, level: Int, operationId: Int, event: Int, status: Int) throws(Error) -> Int
```

## rinzu.log.flush

Checked type: `async(Logger) throws(Error)->Int`.

Error contract: `throws(Error)` (declared).

function · [source](../../sdk/log/src/log.rinzu#L52).

```rinzu
async fn flush(logger: Logger) throws(Error) -> Int
```

## rinzu.log.open

Checked type: `async(String) throws(Error)->Logger`.

Error contract: `throws(Error)` (declared).

function · [source](../../sdk/log/src/log.rinzu#L12).

```rinzu
async fn open(path: String) throws(Error) -> Logger
```
