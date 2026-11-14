# rinzu.sdl2 API reference

Generated from checked public exports and captured source comments.

Native execution requires macOS arm64, SDL2, and `sdl2-config`. See the [SDK ownership guide](../../sdk/sdl2/README.md).

Language: `rinzu-0.5` · target: `aarch64-apple-darwin` · semantics: `rinzu-language-0.5-r85`.

## rinzu.sdl2.Event

Checked type: `Event`.

native-record · [source](../../sdk/sdl2/src/sdl2.rinzu#L7).

```rinzu
record Event
```

### field kind

[source](../../sdk/sdl2/src/sdl2.rinzu#L8).

```rinzu
kind: CUInt32
```

### field windowId

[source](../../sdk/sdl2/src/sdl2.rinzu#L9).

```rinzu
windowId: CUInt32
```

### field shouldQuit

[source](../../sdk/sdl2/src/sdl2.rinzu#L10).

```rinzu
shouldQuit: CBool
```

### field sequence

[source](../../sdk/sdl2/src/sdl2.rinzu#L11).

```rinzu
sequence: Int
```

## rinzu.sdl2.WindowHandle

Checked type: `WindowHandle`.

opaque · [source](../../sdk/sdl2/src/sdl2.rinzu#L5).

```rinzu
opaque WindowHandle;
```

Use the handle on physical Main. Destroy it after its users finish. Closed handles must not be reused.

## rinzu.sdl2.createWindow

Checked type: `native(String,CInt32,CInt32)->WindowHandle`.

native-function · [source](../../sdk/sdl2/src/sdl2.rinzu#L14).

```rinzu
fn createWindow(title: String, width: CInt, height: CInt) -> WindowHandle symbol "rinzu_sdl2_source_window";
```

## rinzu.sdl2.destroyOwnerWindows

Checked type: `native()->CInt32`.

native-function · [source](../../sdk/sdl2/src/sdl2.rinzu#L28).

```rinzu
fn destroyOwnerWindows() -> CInt symbol "rinzu_sdl2_source_destroy_owner_windows";
```

Drains the Main Owner's active window or returns zero when none exists. Check nonzero native status results according to the operation.

## rinzu.sdl2.destroyWindow

Checked type: `native(WindowHandle)->CInt32`.

native-function · [source](../../sdk/sdl2/src/sdl2.rinzu#L26).

```rinzu
fn destroyWindow(window: WindowHandle) -> CInt symbol "rinzu_sdl2_source_destroy";
```

## rinzu.sdl2.isRunning

Checked type: `native(WindowHandle)->Bool`.

native-function · [source](../../sdk/sdl2/src/sdl2.rinzu#L22).

```rinzu
fn isRunning(window: WindowHandle) -> CBool symbol "rinzu_sdl2_source_running";
```

## rinzu.sdl2.liveWindows

Checked type: `native()->Int`.

native-function · [source](../../sdk/sdl2/src/sdl2.rinzu#L30).

```rinzu
fn liveWindows() -> Int symbol "rinzu_sdl2_source_live_windows";
```

Returns zero or one on Main, or minus one on a foreign thread.

## rinzu.sdl2.nextEvent

Checked type: `fn(WindowHandle)->Option<Event>`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/sdl2/src/sdl2.rinzu#L33).

```rinzu
fn nextEvent(window: WindowHandle) -> Option<Event>
```

Returns an independent copy. Keeping an event does not retain an SDL event pointer.

## rinzu.sdl2.render

Checked type: `native(WindowHandle)->CInt32`.

native-function · [source](../../sdk/sdl2/src/sdl2.rinzu#L18).

```rinzu
fn render(window: WindowHandle) -> CInt symbol "rinzu_sdl2_source_render";
```

## rinzu.sdl2.run

Checked type: `async(String,fn(WindowHandle,Event)->Unit,fn(WindowHandle,Int)->Unit,Int)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/sdl2/src/sdl2.rinzu#L41).

```rinzu
async fn run(
    title: String,
    onEvent: fn (WindowHandle, Event) -> Unit,
    onFrame: fn (WindowHandle, Int) -> Unit,
    milliseconds: Int
) -> Int
```

`run` creates a window, dispatches copied events, invokes the synchronous frame callback, renders, and awaits the timer between frames. It returns the frame count and destroys the window in finally. Its callbacks are synchronous; applications that need async event work should compose a managed polling loop and explicit awaits.

## rinzu.sdl2.setTitle

Checked type: `native(WindowHandle,String)->CInt32`.

native-function · [source](../../sdk/sdl2/src/sdl2.rinzu#L20).

```rinzu
fn setTitle(window: WindowHandle, title: String) -> CInt symbol "rinzu_sdl2_source_title";
```

## rinzu.sdl2.stop

Checked type: `native(WindowHandle)->CInt32`.

native-function · [source](../../sdk/sdl2/src/sdl2.rinzu#L24).

```rinzu
fn stop(window: WindowHandle) -> CInt symbol "rinzu_sdl2_source_stop";
```
