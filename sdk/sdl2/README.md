# rinzu.sdl2

Official SDL2 source SDK for macOS arm64. Import it directly from a matching toolchain bundle. SDL2 and `sdl2-config` must be available on the host. Window and renderer operations run on physical Main; one window can be active per SDK image.

createWindow, nextEvent, render, setTitle, isRunning, stop, and destroyWindow support a managed application loop. Event is a copied native record, automatically written back after polling. Retaining an event does not expose or reuse SDL's native event pointer. The existing run callback is synchronous; explicitly await async work in a managed polling loop.

destroyOwnerWindows drains an active window on the Main Owner and returns zero when absent. Call after all users physically settle. Foreign-thread destruction is rejected. GC does not destroy a window. Treat released handles as invalid and do not rely on distinguishing a stale alias after address reuse.

Bound event processing per turn so Main can advance other work. liveWindows measures the active SDK window, not every SDL/framework allocation. This SDK does not provide a native text/IME view equivalent to NSTextView. Native allocation OOM follows fatal termination rules.

[SDK guide](../../docs/libraries/sdk.md) · [API reference](../../docs/reference/sdk-sdl2.md) · [Source](src/sdl2.rinzu)
