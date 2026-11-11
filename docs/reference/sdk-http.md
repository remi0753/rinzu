# rinzu.http API reference

Generated from checked public exports and captured source comments.

Language: `rinzu-0.5` · target: `aarch64-apple-darwin` · semantics: `rinzu-language-0.5-r85`.

## rinzu.http.Client

Checked type: `Client`.

opaque · [source](../../sdk/http/src/http.rinzu#L8).

```rinzu
opaque Client;
```

## rinzu.http.Header

class · [source](../../sdk/http/src/http.rinzu#L66).

```rinzu
class Header
```

### field name

[source](../../sdk/http/src/http.rinzu#L67).

```rinzu
const name: String
```

### field value

[source](../../sdk/http/src/http.rinzu#L68).

```rinzu
const value: ByteArray
```

## rinzu.http.HttpFailure

error · [source](../../sdk/http/src/http.rinzu#L120).

```rinzu
error HttpFailure
```

### field payload

[source](../../sdk/http/src/http.rinzu#L121).

```rinzu
payload: HttpFailureData
```

## rinzu.http.HttpFailureData

class · [source](../../sdk/http/src/http.rinzu#L124).

```rinzu
class HttpFailureData
```

### field cause

[source](../../sdk/http/src/http.rinzu#L125).

```rinzu
const cause: Error
```

### field status

[source](../../sdk/http/src/http.rinzu#L126).

```rinzu
const status: Int
```

### field partial

[source](../../sdk/http/src/http.rinzu#L127).

```rinzu
const partial: ByteArray
```

### field sent

[source](../../sdk/http/src/http.rinzu#L128).

```rinzu
const sent: Int
```

## rinzu.http.PreparedHeaders

class · [source](../../sdk/http/src/http.rinzu#L71).

```rinzu
class PreparedHeaders
```

## rinzu.http.PreparedHeaders.create

Checked type: `fn(List<Header>)->PreparedHeaders`.

Error contract: `throws(HttpFailure)` (inferred).

method · [source](../../sdk/http/src/http.rinzu#L74).

```rinzu
fn create(headers: List<Header>) -> PreparedHeaders
```

## rinzu.http.PreparedHeaders.send

Checked type: `fn(PreparedHeaders,Client,String,String,ByteArray,Int)->Future<ResponseWithHeaders>`.

Error contract: `nothrow` (inferred).

method · [source](../../sdk/http/src/http.rinzu#L78).

```rinzu
fn send(self: PreparedHeaders, client: Client, method: String, url: String, body: ByteArray, timeoutMillis: Int)
        -> Future<ResponseWithHeaders>
```

## rinzu.http.Response

class · [source](../../sdk/http/src/http.rinzu#L61).

```rinzu
class Response
```

### field status

[source](../../sdk/http/src/http.rinzu#L62).

```rinzu
const status: Int
```

### field body

[source](../../sdk/http/src/http.rinzu#L63).

```rinzu
const body: ByteArray
```

## rinzu.http.ResponseWithHeaders

class · [source](../../sdk/http/src/http.rinzu#L89).

```rinzu
class ResponseWithHeaders
```

### field status

[source](../../sdk/http/src/http.rinzu#L90).

```rinzu
const status: Int
```

### field body

[source](../../sdk/http/src/http.rinzu#L91).

```rinzu
const body: ByteArray
```

### field headers

[source](../../sdk/http/src/http.rinzu#L92).

```rinzu
const headers: List<Header>
```

## rinzu.http.ResponseWithHeaders.header

Checked type: `fn(ResponseWithHeaders,String)->Option<ByteArray>`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/http/src/http.rinzu#L94).

```rinzu
fn header(self: ResponseWithHeaders, name: String) -> Option<ByteArray>
```

## rinzu.http.ResponseWithHeaders.headerText

Checked type: `fn(ResponseWithHeaders,String)->Option<String>`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/http/src/http.rinzu#L104).

```rinzu
fn headerText(self: ResponseWithHeaders, name: String) -> Option<String>
```

## rinzu.http.close

Checked type: `async(Client)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/http/src/http.rinzu#L321).

```rinzu
async fn close(client: Client) -> Int
```

## rinzu.http.closeOwnerClients

Checked type: `async()->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/http/src/http.rinzu#L347).

```rinzu
async fn closeOwnerClients() -> Int
```

## rinzu.http.header

Checked type: `fn(String,String)->Header`.

Error contract: `nothrow` (inferred).

function · [source](../../sdk/http/src/http.rinzu#L116).

```rinzu
fn header(name: String, value: String) -> Header
```

## rinzu.http.liveBuffers

Checked type: `native()->Int`.

native-function · [source](../../sdk/http/src/http.rinzu#L56).

```rinzu
fn liveBuffers() -> Int symbol "rinzu_http_live_buffers";
```

## rinzu.http.liveClients

Checked type: `native()->Int`.

native-function · [source](../../sdk/http/src/http.rinzu#L54).

```rinzu
fn liveClients() -> Int symbol "rinzu_http_live_clients";
```

## rinzu.http.newClient

Checked type: `native()->Client`.

native-function · [source](../../sdk/http/src/http.rinzu#L12).

```rinzu
fn newClient() -> Client symbol "rinzu_http_new";
```

## rinzu.http.request

Checked type: `async(Client,String,ByteArray,Bool,Int)->Response`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/http/src/http.rinzu#L177).

```rinzu
async fn request(client: Client, url: String, body: ByteArray, post: Bool, timeoutMillis: Int) -> Response
```

## rinzu.http.retainedControlBytes

Checked type: `native()->Int`.

native-function · [source](../../sdk/http/src/http.rinzu#L58).

```rinzu
fn retainedControlBytes() -> Int symbol "rinzu_http_retained_controls";
```

## rinzu.http.send

Checked type: `fn(Client,String,String,List<Header>,ByteArray,Int)->Future<ResponseWithHeaders>`.

Error contract: `nothrow` (inferred).

function · [source](../../sdk/http/src/http.rinzu#L277).

```rinzu
fn send(client: Client, method: String, url: String, headers: List<Header>, body: ByteArray, timeoutMillis: Int)
    -> Future<ResponseWithHeaders>
```

## rinzu.http.server.Application

class · [source](../../sdk/http/src/server/application.rinzu#L235).

```rinzu
class Application
```

### field registry

[source](../../sdk/http/src/server/application.rinzu#L236).

```rinzu
const registry: Registry
```

### field scope

[source](../../sdk/http/src/server/application.rinzu#L237).

```rinzu
const scope: Scope
```

## rinzu.http.server.Application.compile

Checked type: `fn(server.application::Application)->server.application::Compiled`.

Error contract: `throws(rinzu.list::LengthOverflow, server.router::ConfigurationError)` (inferred).

method · [source](../../sdk/http/src/server/application.rinzu#L373).

```rinzu
fn compile(self: Application) -> Compiled
```

## rinzu.http.server.Application.get

Checked type: `fn(server.application::Application,String,$rinzu_closure_f9dd8846ac4098b0)->Unit`.

Error contract: `throws(rinzu.list::LengthOverflow, rinzu.map::LengthOverflow, server.router::ConfigurationError, server.url::InputError)` (inferred).

method · [source](../../sdk/http/src/server/application.rinzu#L268).

```rinzu
fn get(self: Application, path: String, handler: Handler)
```

## rinzu.http.server.Application.getSync

Checked type: `fn(server.application::Application,String,$rinzu_closure_fa2f60f4bd8d2116)->Unit`.

Error contract: `throws(rinzu.list::LengthOverflow, rinzu.map::LengthOverflow, server.router::ConfigurationError, server.url::InputError)` (inferred).

method · [source](../../sdk/http/src/server/application.rinzu#L276).

```rinzu
fn getSync(self: Application, path: String, handler: SyncHandler)
```

## rinzu.http.server.Application.group

Checked type: `fn(server.application::Application,String)->server.application::Application`.

Error contract: `throws(rinzu.list::LengthOverflow, server.router::ConfigurationError, server.url::InputError)` (inferred).

method · [source](../../sdk/http/src/server/application.rinzu#L305).

```rinzu
fn group(self: Application, prefix: String) -> Application
```

## rinzu.http.server.Application.mount

Checked type: `fn(server.application::Application,String,server.application::Application)->Unit`.

Error contract: `throws(rinzu.list::LengthOverflow, rinzu.map::LengthOverflow, server.router::ConfigurationError, server.url::InputError)` (inferred).

method · [source](../../sdk/http/src/server/application.rinzu#L320).

```rinzu
fn mount(self: Application, prefix: String, child: Application)
```

## rinzu.http.server.Application.on

Checked type: `fn(server.application::Application,String,String,$rinzu_closure_f9dd8846ac4098b0)->Unit`.

Error contract: `throws(rinzu.list::LengthOverflow, rinzu.map::LengthOverflow, server.router::ConfigurationError, server.url::InputError)` (inferred).

method · [source](../../sdk/http/src/server/application.rinzu#L239).

```rinzu
fn on(self: Application, method: String, path: String, handler: Handler)
```

## rinzu.http.server.Application.onAsync

Checked type: `fn(server.application::Application,String,String,$rinzu_closure_f9dd8846ac4098b0)->Unit`.

Error contract: `throws(rinzu.list::LengthOverflow, rinzu.map::LengthOverflow, server.router::ConfigurationError, server.url::InputError)` (inferred).

method · [source](../../sdk/http/src/server/application.rinzu#L243).

```rinzu
fn onAsync(self: Application, method: String, path: String, handler: Handler)
```

## rinzu.http.server.Application.onError

Checked type: `fn(server.application::Application,$rinzu_closure_b4581a1efc34167d)->Unit`.

Error contract: `throws(server.router::ConfigurationError)` (inferred).

method · [source](../../sdk/http/src/server/application.rinzu#L300).

```rinzu
fn onError(self: Application, handler: ErrorHandler)
```

## rinzu.http.server.Application.onSync

Checked type: `fn(server.application::Application,String,String,$rinzu_closure_fa2f60f4bd8d2116)->Unit`.

Error contract: `throws(rinzu.list::LengthOverflow, rinzu.map::LengthOverflow, server.router::ConfigurationError, server.url::InputError)` (inferred).

method · [source](../../sdk/http/src/server/application.rinzu#L247).

```rinzu
fn onSync(self: Application, method: String, path: String, handler: SyncHandler)
```

## rinzu.http.server.Application.post

Checked type: `fn(server.application::Application,String,$rinzu_closure_f9dd8846ac4098b0)->Unit`.

Error contract: `throws(rinzu.list::LengthOverflow, rinzu.map::LengthOverflow, server.router::ConfigurationError, server.url::InputError)` (inferred).

method · [source](../../sdk/http/src/server/application.rinzu#L272).

```rinzu
fn post(self: Application, path: String, handler: Handler)
```

## rinzu.http.server.Application.postSync

Checked type: `fn(server.application::Application,String,$rinzu_closure_fa2f60f4bd8d2116)->Unit`.

Error contract: `throws(rinzu.list::LengthOverflow, rinzu.map::LengthOverflow, server.router::ConfigurationError, server.url::InputError)` (inferred).

method · [source](../../sdk/http/src/server/application.rinzu#L280).

```rinzu
fn postSync(self: Application, path: String, handler: SyncHandler)
```

## rinzu.http.server.Application.use

Checked type: `fn(server.application::Application,$rinzu_closure_c626408049bbd134)->Unit`.

Error contract: `throws(rinzu.list::LengthOverflow, server.router::ConfigurationError)` (inferred).

method · [source](../../sdk/http/src/server/application.rinzu#L284).

```rinzu
fn use(self: Application, middleware: Middleware)
```

## rinzu.http.server.Application.useSync

Checked type: `fn(server.application::Application,$rinzu_closure_aff1fa692d7db4f1)->Unit`.

Error contract: `throws(rinzu.list::LengthOverflow, server.router::ConfigurationError)` (inferred).

method · [source](../../sdk/http/src/server/application.rinzu#L288).

```rinzu
fn useSync(self: Application, middleware: SyncMiddleware)
```

## rinzu.http.server.Compiled

class · [source](../../sdk/http/src/server/application.rinzu#L408).

```rinzu
class Compiled
```

### field registry

[source](../../sdk/http/src/server/application.rinzu#L409).

```rinzu
const registry: Registry
```

## rinzu.http.server.Compiled.dispatch

Checked type: `fn(server.application::Compiled,server.application::Request)->server.application::Dispatch`.

Error contract: `nothrow` (inferred).

method · [source](../../sdk/http/src/server/application.rinzu#L424).

```rinzu
fn dispatch(self: Compiled, input: Request) -> Dispatch
```

## rinzu.http.server.Compiled.handle

Checked type: `fn(server.application::Compiled,server.application::Request)->Future<server.application::Response>`.

Error contract: `nothrow` (inferred).

method · [source](../../sdk/http/src/server/application.rinzu#L460).

```rinzu
fn handle(self: Compiled, input: Request) -> Future<Response>
```

## rinzu.http.server.Compiled.inject

Checked type: `fn(server.application::Compiled,String,String)->Future<server.application::Response>`.

Error contract: `nothrow` (inferred).

method · [source](../../sdk/http/src/server/application.rinzu#L411).

```rinzu
fn inject(self: Compiled, method: String, target: String) -> Future<Response>
```

## rinzu.http.server.Compiled.injectBody

Checked type: `async(server.application::Compiled,String,String,ByteArray)->server.application::Response`.

Error contract: `nothrow` (inferred).

method · [source](../../sdk/http/src/server/application.rinzu#L415).

```rinzu
async fn injectBody(self: Compiled, method: String, target: String, body: ByteArray) -> Response
```

## rinzu.http.server.ConfigurationError

error · [source](../../sdk/http/src/server/router.rinzu#L6).

```rinzu
error ConfigurationError
```

## rinzu.http.server.Dispatch

enum · [source](../../sdk/http/src/server/application.rinzu#L465).

```rinzu
enum Dispatch
```

### variant Ready

[source](../../sdk/http/src/server/application.rinzu#L466).

```rinzu
Ready(response: Response)
```

### variant Pending

[source](../../sdk/http/src/server/application.rinzu#L467).

```rinzu
Pending(work: PendingDispatch)
```

## rinzu.http.server.Endpoint

class · [source](../../sdk/http/src/server/application.rinzu#L204).

```rinzu
class Endpoint
```

### field method

[source](../../sdk/http/src/server/application.rinzu#L205).

```rinzu
const method: String
```

### field path

[source](../../sdk/http/src/server/application.rinzu#L206).

```rinzu
const path: String
```

### field names

[source](../../sdk/http/src/server/application.rinzu#L207).

```rinzu
const names: List<String>
```

### field handler

[source](../../sdk/http/src/server/application.rinzu#L208).

```rinzu
const handler: RouteHandler
```

### field scope

[source](../../sdk/http/src/server/application.rinzu#L209).

```rinzu
const scope: Scope
```

### field pipeline

[source](../../sdk/http/src/server/application.rinzu#L210).

```rinzu
pipeline: Option<Pipeline>
```

## rinzu.http.server.ErrorHandler

Checked type: `fn(server.application::Request,List<Error>)->Future<server.application::Response>`.

type-alias · [source](../../sdk/http/src/server/application.rinzu#L175).

```rinzu
type ErrorHandler = fn (Request, List<Error>) -> Future<Response>;
```

## rinzu.http.server.ErrorPolicy

class · [source](../../sdk/http/src/server/application.rinzu#L186).

```rinzu
class ErrorPolicy
```

### field apply

[source](../../sdk/http/src/server/application.rinzu#L187).

```rinzu
const apply: ErrorHandler
```

## rinzu.http.server.Handler

Checked type: `fn(server.application::Request)->Future<server.application::Response>`.

type-alias · [source](../../sdk/http/src/server/application.rinzu#L163).

```rinzu
type Handler = fn (Request) -> Future<Response>;
```

## rinzu.http.server.Header

class · [source](../../sdk/http/src/server/application.rinzu#L8).

```rinzu
class Header
```

### field name

[source](../../sdk/http/src/server/application.rinzu#L9).

```rinzu
const name: String
```

### field value

[source](../../sdk/http/src/server/application.rinzu#L10).

```rinzu
const value: String
```

## rinzu.http.server.InputError

error · [source](../../sdk/http/src/server/url.rinzu#L5).

```rinzu
error InputError
```

### field payload

[source](../../sdk/http/src/server/url.rinzu#L6).

```rinzu
payload: Int
```

## rinzu.http.server.Layer

class · [source](../../sdk/http/src/server/application.rinzu#L177).

```rinzu
class Layer
```

### field apply

[source](../../sdk/http/src/server/application.rinzu#L178).

```rinzu
const apply: Middleware
```

## rinzu.http.server.Middleware

Checked type: `fn(server.application::Request,server.application::Next)->Future<server.application::Response>`.

type-alias · [source](../../sdk/http/src/server/application.rinzu#L173).

```rinzu
type Middleware = fn (Request, Next) -> Future<Response>;
```

## rinzu.http.server.MiddlewareLayer

enum · [source](../../sdk/http/src/server/application.rinzu#L181).

```rinzu
enum MiddlewareLayer
```

### variant Sync

[source](../../sdk/http/src/server/application.rinzu#L182).

```rinzu
Sync(apply: SyncMiddleware)
```

### variant Async

[source](../../sdk/http/src/server/application.rinzu#L183).

```rinzu
Async(layer: Layer)
```

## rinzu.http.server.Next

class · [source](../../sdk/http/src/server/application.rinzu#L220).

```rinzu
class Next
```

### field request

[source](../../sdk/http/src/server/application.rinzu#L221).

```rinzu
const request: Request
```

### field handler

[source](../../sdk/http/src/server/application.rinzu#L222).

```rinzu
const handler: Handler
```

### field used

[source](../../sdk/http/src/server/application.rinzu#L223).

```rinzu
used: Bool
```

### field active

[source](../../sdk/http/src/server/application.rinzu#L224).

```rinzu
active: Bool
```

## rinzu.http.server.Next.run

Checked type: `async(server.application::Next)->server.application::Response`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/http/src/server/application.rinzu#L226).

```rinzu
async fn run(self: Next) -> Response
```

## rinzu.http.server.Node

class · [source](../../sdk/http/src/server/router.rinzu#L18).

```rinzu
class Node
```

### field edges

[source](../../sdk/http/src/server/router.rinzu#L19).

```rinzu
edges: Map<String, Node>
```

### field parameter

[source](../../sdk/http/src/server/router.rinzu#L20).

```rinzu
parameter: Option<Node>
```

### field endpoint

[source](../../sdk/http/src/server/router.rinzu#L21).

```rinzu
endpoint: Int
```

### field wildcard

[source](../../sdk/http/src/server/router.rinzu#L22).

```rinzu
wildcard: Int
```

## rinzu.http.server.PendingDispatch

class · [source](../../sdk/http/src/server/application.rinzu#L476).

```rinzu
class PendingDispatch
```

### field registry

[source](../../sdk/http/src/server/application.rinzu#L477).

```rinzu
const registry: Registry
```

### field input

[source](../../sdk/http/src/server/application.rinzu#L478).

```rinzu
const input: Request
```

### field chain

[source](../../sdk/http/src/server/application.rinzu#L479).

```rinzu
const chain: Pipeline
```

### field failure

[source](../../sdk/http/src/server/application.rinzu#L480).

```rinzu
const failure: Option<List<Error>>
```

### field started

[source](../../sdk/http/src/server/application.rinzu#L481).

```rinzu
started: Bool
```

## rinzu.http.server.PendingDispatch.run

Checked type: `fn(server.application::PendingDispatch)->Future<server.application::Response>`.

Error contract: `nothrow` (inferred).

method · [source](../../sdk/http/src/server/application.rinzu#L483).

```rinzu
fn run(self: PendingDispatch) -> Future<Response>
```

## rinzu.http.server.Pipeline

class · [source](../../sdk/http/src/server/application.rinzu#L190).

```rinzu
class Pipeline
```

### field handler

[source](../../sdk/http/src/server/application.rinzu#L191).

```rinzu
const handler: RouteHandler
```

### field errors

[source](../../sdk/http/src/server/application.rinzu#L192).

```rinzu
const errors: ErrorPolicy
```

## rinzu.http.server.Query

class · [source](../../sdk/http/src/server/application.rinzu#L13).

```rinzu
class Query
```

### field key

[source](../../sdk/http/src/server/application.rinzu#L14).

```rinzu
const key: String
```

### field value

[source](../../sdk/http/src/server/application.rinzu#L15).

```rinzu
const value: String
```

## rinzu.http.server.Registry

class · [source](../../sdk/http/src/server/application.rinzu#L213).

```rinzu
class Registry
```

### field router

[source](../../sdk/http/src/server/application.rinzu#L214).

```rinzu
const router: route.Router
```

### field endpoints

[source](../../sdk/http/src/server/application.rinzu#L215).

```rinzu
endpoints: List<Endpoint>
```

### field scopes

[source](../../sdk/http/src/server/application.rinzu#L216).

```rinzu
scopes: List<Scope>
```

### field sealed

[source](../../sdk/http/src/server/application.rinzu#L217).

```rinzu
sealed: Bool
```

## rinzu.http.server.Request

class · [source](../../sdk/http/src/server/application.rinzu#L18).

```rinzu
class Request
```

### field method

[source](../../sdk/http/src/server/application.rinzu#L19).

```rinzu
const method: String
```

### field target

[source](../../sdk/http/src/server/application.rinzu#L20).

```rinzu
const target: String
```

### field path

[source](../../sdk/http/src/server/application.rinzu#L21).

```rinzu
const path: String
```

### field rawQuery

[source](../../sdk/http/src/server/application.rinzu#L22).

```rinzu
const rawQuery: String
```

### field headers

[source](../../sdk/http/src/server/application.rinzu#L23).

```rinzu
const headers: List<Header>
```

### field body

[source](../../sdk/http/src/server/application.rinzu#L24).

```rinzu
const body: ByteArray
```

### field names

[source](../../sdk/http/src/server/application.rinzu#L25).

```rinzu
names: List<String>
```

### field values

[source](../../sdk/http/src/server/application.rinzu#L26).

```rinzu
values: List<route.Span>
```

### field queryCache

[source](../../sdk/http/src/server/application.rinzu#L27).

```rinzu
queryCache: Option<List<Query>>
```

### field textCache

[source](../../sdk/http/src/server/application.rinzu#L28).

```rinzu
textCache: Option<String>
```

### field dispatched

[source](../../sdk/http/src/server/application.rinzu#L29).

```rinzu
dispatched: Bool
```

## rinzu.http.server.Request.header

Checked type: `fn(server.application::Request,String)->Option<String>`.

Error contract: `nothrow` (inferred).

method · [source](../../sdk/http/src/server/application.rinzu#L110).

```rinzu
fn header(self: Request, name: String) -> Option<String>
```

## rinzu.http.server.Request.param

Checked type: `fn(server.application::Request,String)->String`.

Error contract: `throws(server.router::ConfigurationError, server.url::InputError)` (inferred).

method · [source](../../sdk/http/src/server/application.rinzu#L31).

```rinzu
fn param(self: Request, name: String) -> String
```

## rinzu.http.server.Request.query

Checked type: `fn(server.application::Request,String)->Option<String>`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/http/src/server/application.rinzu#L70).

```rinzu
fn query(self: Request, name: String) -> Option<String>
```

## rinzu.http.server.Request.queryAll

Checked type: `fn(server.application::Request,String)->List<String>`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/http/src/server/application.rinzu#L52).

```rinzu
fn queryAll(self: Request, name: String) -> List<String>
```

## rinzu.http.server.Request.queryInt

Checked type: `fn(server.application::Request,String,Int)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/http/src/server/application.rinzu#L87).

```rinzu
fn queryInt(self: Request, name: String, defaultValue: Int) -> Int
```

## rinzu.http.server.Request.text

Checked type: `fn(server.application::Request)->String`.

Error contract: `throws(server.url::InputError)` (inferred).

method · [source](../../sdk/http/src/server/application.rinzu#L126).

```rinzu
fn text(self: Request) -> String
```

## rinzu.http.server.Response

class · [source](../../sdk/http/src/server/application.rinzu#L144).

```rinzu
class Response
```

### field status

[source](../../sdk/http/src/server/application.rinzu#L145).

```rinzu
const status: Int
```

### field contentType

[source](../../sdk/http/src/server/application.rinzu#L146).

```rinzu
const contentType: String
```

### field headers

[source](../../sdk/http/src/server/application.rinzu#L147).

```rinzu
const headers: List<Header>
```

### field body

[source](../../sdk/http/src/server/application.rinzu#L148).

```rinzu
const body: ByteArray
```

## rinzu.http.server.Response.withHeader

Checked type: `fn(server.application::Response,String,String)->server.application::Response`.

Error contract: `throws(rinzu.list::LengthOverflow, server.router::ConfigurationError)` (inferred).

method · [source](../../sdk/http/src/server/application.rinzu#L150).

```rinzu
fn withHeader(self: Response, name: String, value: String) -> Response
```

## rinzu.http.server.RouteHandler

enum · [source](../../sdk/http/src/server/application.rinzu#L167).

```rinzu
enum RouteHandler
```

### variant Sync

[source](../../sdk/http/src/server/application.rinzu#L168).

```rinzu
Sync(apply: SyncHandler)
```

### variant Async

[source](../../sdk/http/src/server/application.rinzu#L169).

```rinzu
Async(apply: Handler)
```

### variant Filtered

[source](../../sdk/http/src/server/application.rinzu#L170).

```rinzu
Filtered(filters: List<SyncMiddleware>, apply: Handler)
```

## rinzu.http.server.Router

class · [source](../../sdk/http/src/server/router.rinzu#L25).

```rinzu
class Router
```

### field roots

[source](../../sdk/http/src/server/router.rinzu#L26).

```rinzu
roots: Map<String, Node>
```

## rinzu.http.server.Scope

class · [source](../../sdk/http/src/server/application.rinzu#L195).

```rinzu
class Scope
```

### field parent

[source](../../sdk/http/src/server/application.rinzu#L196).

```rinzu
parent: Option<Scope>
```

### field prefix

[source](../../sdk/http/src/server/application.rinzu#L197).

```rinzu
const prefix: String
```

### field layers

[source](../../sdk/http/src/server/application.rinzu#L198).

```rinzu
layers: List<MiddlewareLayer>
```

### field errors

[source](../../sdk/http/src/server/application.rinzu#L199).

```rinzu
errors: Option<ErrorPolicy>
```

### field flat

[source](../../sdk/http/src/server/application.rinzu#L200).

```rinzu
flat: List<MiddlewareLayer>
```

### field policy

[source](../../sdk/http/src/server/application.rinzu#L201).

```rinzu
policy: ErrorPolicy
```

## rinzu.http.server.Server

class · [source](../../sdk/http/src/server/http1.rinzu#L397).

```rinzu
class Server
```

### field port

[source](../../sdk/http/src/server/http1.rinzu#L398).

```rinzu
const port: Int
```

## rinzu.http.server.Server.close

Checked type: `async(server.http1::Server)->Unit`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/http/src/server/http1.rinzu#L531).

```rinzu
async fn close(self: Server)
```

## rinzu.http.server.Server.completedResponses

Checked type: `fn(server.http1::Server)->Int`.

Error contract: `nothrow` (inferred).

method · [source](../../sdk/http/src/server/http1.rinzu#L440).

```rinzu
fn completedResponses(self: Server) -> Int
```

## rinzu.http.server.Server.listen

Checked type: `async(server.application::Compiled,String,Int,server.http1::ServerOptions)->server.http1::Server`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/http/src/server/http1.rinzu#L406).

```rinzu
async fn listen(application: http.Compiled, host: String, port: Int, options: ServerOptions) -> Server
```

## rinzu.http.server.Server.serve

Checked type: `fn(server.http1::Server)->Future<Unit>`.

Error contract: `nothrow` (inferred).

method · [source](../../sdk/http/src/server/http1.rinzu#L448).

```rinzu
fn serve(self: Server) -> Future<Unit>
```

## rinzu.http.server.Server.serveUntil

Checked type: `async(server.http1::Server,$rinzu_closure_0a4a4e7dcaa0da98)->Unit`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/http/src/server/http1.rinzu#L456).

```rinzu
async fn serveUntil(self: Server, stopRequested: fn () -> Bool)
```

## rinzu.http.server.Server.stop

Checked type: `fn(server.http1::Server)->Unit`.

Error contract: `nothrow` (inferred).

method · [source](../../sdk/http/src/server/http1.rinzu#L444).

```rinzu
fn stop(self: Server)
```

## rinzu.http.server.ServerOptions

class · [source](../../sdk/http/src/server/http1.rinzu#L128).

```rinzu
class ServerOptions
```

### field backlog

[source](../../sdk/http/src/server/http1.rinzu#L129).

```rinzu
const backlog: Int
```

### field maxConnections

[source](../../sdk/http/src/server/http1.rinzu#L130).

```rinzu
const maxConnections: Int
```

### field requestTimeoutMillis

[source](../../sdk/http/src/server/http1.rinzu#L131).

```rinzu
const requestTimeoutMillis: Int
```

### field keepAliveTimeoutMillis

[source](../../sdk/http/src/server/http1.rinzu#L132).

```rinzu
const keepAliveTimeoutMillis: Int
```

### field writeTimeoutMillis

[source](../../sdk/http/src/server/http1.rinzu#L133).

```rinzu
const writeTimeoutMillis: Int
```

### field shutdownTimeoutMillis

[source](../../sdk/http/src/server/http1.rinzu#L134).

```rinzu
const shutdownTimeoutMillis: Int
```

## rinzu.http.server.Span

class · [source](../../sdk/http/src/server/router.rinzu#L8).

```rinzu
class Span
```

### field start

[source](../../sdk/http/src/server/router.rinzu#L9).

```rinzu
const start: Int
```

### field end

[source](../../sdk/http/src/server/router.rinzu#L10).

```rinzu
const end: Int
```

## rinzu.http.server.SyncHandler

Checked type: `fn(server.application::Request)->server.application::Response`.

type-alias · [source](../../sdk/http/src/server/application.rinzu#L161).

```rinzu
type SyncHandler = fn (Request) -> Response;
```

## rinzu.http.server.SyncMiddleware

Checked type: `fn(server.application::Request)->Option<server.application::Response>`.

type-alias · [source](../../sdk/http/src/server/application.rinzu#L165).

```rinzu
type SyncMiddleware = fn (Request) -> Option<Response>;
```

## rinzu.http.server.application

Checked type: `fn()->server.application::Application`.

Error contract: `nothrow` (inferred).

function · [source](../../sdk/http/src/server/application.rinzu#L613).

```rinzu
fn application() -> Application
```

## rinzu.http.server.binary

Checked type: `fn(Int,ByteArray)->server.application::Response`.

Error contract: `nothrow` (inferred).

function · [source](../../sdk/http/src/server/application.rinzu#L832).

```rinzu
fn binary(status: Int, value: ByteArray) -> Response
```

## rinzu.http.server.checked

Checked type: `fn(server.application::Response)->server.application::Response`.

Error contract: `throws(server.router::ConfigurationError)` (inferred).

function · [source](../../sdk/http/src/server/application.rinzu#L876).

```rinzu
fn checked(value: Response) -> Response
```

## rinzu.http.server.defaultServerOptions

Checked type: `fn()->server.http1::ServerOptions`.

Error contract: `nothrow` (inferred).

function · [source](../../sdk/http/src/server/http1.rinzu#L137).

```rinzu
fn defaultServerOptions() -> ServerOptions
```

## rinzu.http.server.request

Checked type: `fn(String,String,List<server.application::Header>,ByteArray)->server.application::Request`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/http/src/server/application.rinzu#L912).

```rinzu
fn request(method: String, target: String, headers: List<Header>, body: ByteArray) -> Request
```

## rinzu.http.server.text

Checked type: `fn(Int,String)->server.application::Response`.

Error contract: `nothrow` (inferred).

function · [source](../../sdk/http/src/server/application.rinzu#L828).

```rinzu
fn text(status: Int, value: String) -> Response
```
