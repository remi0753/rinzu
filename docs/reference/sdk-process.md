# rinzu.process API reference

Generated from checked public exports and captured source comments.

Language: `rinzu-0.5` · target: `aarch64-apple-darwin` · semantics: `rinzu-language-0.5-r81`.

## rinzu.process.InvalidArgumentEncoding

error · [source](../../sdk/process/src/process.rinzu#L3).

```rinzu
error InvalidArgumentEncoding
```

### field payload

[source](../../sdk/process/src/process.rinzu#L4).

```rinzu
payload: { index: Int, byteOffset: Int }
```

## rinzu.process.arguments

Checked type: `fn()->List<String>`.

Error contract: `throws InvalidArgumentEncoding` (declared).

function · [source](../../sdk/process/src/process.rinzu#L12).

```rinzu
fn arguments() -> List<String> throws InvalidArgumentEncoding
```

## rinzu.process.environment

Checked type: `fn(String)->Option<String>`.

Error contract: `throws Error` (inferred).

function · [source](../../sdk/process/src/process.rinzu#L39).

```rinzu
fn environment(name: String) -> Option<String>
```

## rinzu.process.identifier

Checked type: `native()->Int`.

native-function · [source](../../sdk/process/src/process.rinzu#L36).

```rinzu
fn identifier() -> Int symbol "rinzu_process_identifier";
```

## rinzu.process.integer

Checked type: `fn(String,Int,Int,Int)->Int`.

Error contract: `throws Error` (inferred).

function · [source](../../sdk/process/src/process.rinzu#L76).

```rinzu
fn integer(name: String, defaultValue: Int, minimum: Int, maximum: Int) -> Int
```

## rinzu.process.setExitStatus

Checked type: `fn(Int)->Bool`.

Error contract: `nothrow` (inferred).

function · [source](../../sdk/process/src/process.rinzu#L100).

```rinzu
fn setExitStatus(status: Int) -> Bool
```

## rinzu.process.setting

Checked type: `fn(String,String)->String`.

Error contract: `throws Error` (inferred).

function · [source](../../sdk/process/src/process.rinzu#L67).

```rinzu
fn setting(name: String, defaultValue: String) -> String
```

## rinzu.process.writeErrorLine

Checked type: `fn(String)->Int`.

Error contract: `throws Error` (inferred).

function · [source](../../sdk/process/src/process.rinzu#L104).

```rinzu
fn writeErrorLine(text: String) -> Int
```
