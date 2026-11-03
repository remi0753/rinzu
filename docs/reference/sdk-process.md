# rinzu.process API reference

Generated from checked public exports and captured source comments.

Language: `rinzu-0.5` · target: `aarch64-apple-darwin` · semantics: `rinzu-language-0.5-r76`.

## rinzu.process.environment

Checked type: `fn(String)->Option<String>`.

function · [source](../../sdk/process/src/process.rinzu#L11).

```rinzu
fn environment(name: String) -> Option<String>
```

## rinzu.process.identifier

Checked type: `native()->Int`.

native-function · [source](../../sdk/process/src/process.rinzu#L8).

```rinzu
fn identifier() -> Int symbol "rinzu_process_identifier";
```

## rinzu.process.integer

Checked type: `fn(String,Int,Int,Int)->Int`.

function · [source](../../sdk/process/src/process.rinzu#L51).

```rinzu
fn integer(name: String, defaultValue: Int, minimum: Int, maximum: Int) -> Int
```

## rinzu.process.setExitStatus

Checked type: `fn(Int)->Bool`.

function · [source](../../sdk/process/src/process.rinzu#L72).

```rinzu
fn setExitStatus(status: Int) -> Bool
```

## rinzu.process.setting

Checked type: `fn(String,String)->String`.

function · [source](../../sdk/process/src/process.rinzu#L39).

```rinzu
fn setting(name: String, defaultValue: String) -> String
```

## rinzu.process.writeErrorLine

Checked type: `fn(String)->Int`.

function · [source](../../sdk/process/src/process.rinzu#L76).

```rinzu
fn writeErrorLine(text: String) -> Int
```
