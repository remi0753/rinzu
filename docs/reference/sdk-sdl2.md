# rinzu.sdl2 API reference

Public language 0.5 declarations from [sdl2.rinzu](../../sdk/sdl2/src/sdl2.rinzu). Native execution requires macOS arm64, SDL2, and sdl2-config. See the [SDK ownership guide](../../sdk/sdl2/README.md).

## Types

`WindowHandle` is opaque and non-Transferable. Create it through createWindow, use it only on physical Main, and explicitly destroy it after its users finish. It has no public constructor or fields.

`Event` is a native record with these fields:

```rinzu
record Event {
    kind: CUInt32,
    windowId: CUInt32,
    shouldQuit: CBool,
    sequence: Int
}
```

nextEvent returns an independent copied Event. Keeping an event does not retain an SDL event pointer.

## Window operations

| Public function | Result |
| --- | --- |
| `createWindow(title: String, width: CInt, height: CInt)` | WindowHandle |
| `nextEvent(window: WindowHandle)` | Option<Event> |
| `render(window: WindowHandle)` | CInt status |
| `setTitle(window: WindowHandle, title: String)` | CInt status |
| `isRunning(window: WindowHandle)` | CBool |
| `stop(window: WindowHandle)` | CInt status |
| `destroyWindow(window: WindowHandle)` | CInt status |
| `destroyOwnerWindows()` | CInt status |
| `liveWindows()` | Int count |

A nonzero returned native status must be handled according to the operation. One window may be active per SDK image. destroyOwnerWindows drains the Main Owner's active window, or returns zero when none exists. liveWindows reports zero or one on Main and minus one on a foreign thread. Closed/stale handles must not be reused.

## Managed run loop

```rinzu
async fn run(
    title: String,
    onEvent: fn (WindowHandle, Event) -> Unit,
    onFrame: fn (WindowHandle, Int) -> Unit,
    milliseconds: Int
) -> Int
```

run creates a window, dispatches copied events, invokes the synchronous frame callback, renders, and awaits the timer between frames. It returns the frame count and destroys the window in finally. Its callbacks are synchronous; applications that need async event work should compose a managed polling loop and explicit awaits.
