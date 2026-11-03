# rinzu.http API reference

Generated from checked public exports and captured source comments.

Language: `rinzu-0.5` · target: `aarch64-apple-darwin` · semantics: `rinzu-language-0.5-r76`.

## rinzu.http.Client

Checked type: `Client`.

opaque · [source](../../sdk/http/src/http.rinzu#L2).

```rinzu
opaque Client;
```

## rinzu.http.HttpFailure

error · [source](../../sdk/http/src/http.rinzu#L50).

```rinzu
error HttpFailure
```

### field payload

[source](../../sdk/http/src/http.rinzu#L51).

```rinzu
payload: HttpFailureData
```

## rinzu.http.HttpFailureData

class · [source](../../sdk/http/src/http.rinzu#L54).

```rinzu
class HttpFailureData
```

### field cause

[source](../../sdk/http/src/http.rinzu#L55).

```rinzu
const cause: Error
```

### field status

[source](../../sdk/http/src/http.rinzu#L56).

```rinzu
const status: Int
```

### field partial

[source](../../sdk/http/src/http.rinzu#L57).

```rinzu
const partial: ByteArray
```

### field sent

[source](../../sdk/http/src/http.rinzu#L58).

```rinzu
const sent: Int
```

## rinzu.http.Response

class · [source](../../sdk/http/src/http.rinzu#L45).

```rinzu
class Response
```

### field status

[source](../../sdk/http/src/http.rinzu#L46).

```rinzu
const status: Int
```

### field body

[source](../../sdk/http/src/http.rinzu#L47).

```rinzu
const body: ByteArray
```

## rinzu.http.close

Checked type: `async(Client)->Int`.

function · [source](../../sdk/http/src/http.rinzu#L125).

```rinzu
async fn close(client: Client) -> Int
```

## rinzu.http.closeOwnerClients

Checked type: `async()->Int`.

function · [source](../../sdk/http/src/http.rinzu#L146).

```rinzu
async fn closeOwnerClients() -> Int
```

## rinzu.http.liveBuffers

Checked type: `native()->Int`.

native-function · [source](../../sdk/http/src/http.rinzu#L40).

```rinzu
fn liveBuffers() -> Int symbol "rinzu_http_live_buffers";
```

## rinzu.http.liveClients

Checked type: `native()->Int`.

native-function · [source](../../sdk/http/src/http.rinzu#L38).

```rinzu
fn liveClients() -> Int symbol "rinzu_http_live_clients";
```

## rinzu.http.newClient

Checked type: `native()->Client`.

native-function · [source](../../sdk/http/src/http.rinzu#L6).

```rinzu
fn newClient() -> Client symbol "rinzu_http_new";
```

## rinzu.http.request

Checked type: `async(Client,String,ByteArray,Bool,Int)->Response`.

function · [source](../../sdk/http/src/http.rinzu#L85).

```rinzu
async fn request(client: Client, url: String, body: ByteArray, post: Bool, timeoutMillis: Int) -> Response
```

## rinzu.http.retainedControlBytes

Checked type: `native()->Int`.

native-function · [source](../../sdk/http/src/http.rinzu#L42).

```rinzu
fn retainedControlBytes() -> Int symbol "rinzu_http_retained_controls";
```
