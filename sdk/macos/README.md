# rinzu.macos

Official AppKit and Metal SDK for language 0.5 on macOS arm64. A desktop session is required. The API uses a common Session/Window/View model, owned events, managed async handlers/jobs, GPU operations, documents, and sheets.

[GUI guide](../../docs/libraries/macos.md) · [Complete API reference](../../docs/reference/sdk-macos.md) · [Source](src/macos.rinzu)

## Start and stop an application

Import `rinzu.macos` directly from a matching toolchain bundle, or declare a local path to the SDK. application(title,width,height,body) owns the OS loop and invokes an async body receiving UiApplication. applicationWithSignals also registers cooperative process signals. Use app.primary for the initial UiScope and app.session for session-wide operations.

A scope owns its window, resources, and jobs. Dispatch owned UiEvent values in managed async handlers. Request stop when closing or quitting, stop intake, and await jobs/native/GPU completion before releasing their resources. Application shutdown is explicit; a native window does not become a transferable Worker value.

Session/Window/View and rendering resources are managed classes wrapping exported opaque handle types. Opaque handles have no methods or constructors and are used at the native ABI. Rect, Color, Event, and statistics are native scalar records with automatic writeback.

The low-level run handler has type fn(RunContext,Event)->Unit. RunContext is a language record containing session. Native code keeps the context through actual call/callback settlement and uses its checked fallback. Do not retain arbitrary managed callbacks outside supported call lifetimes.

## Controls and events

The SDK includes labels, buttons, text fields/views, menus, checkboxes, layout helpers, accessibility, tooltips, themes, focus/scroll state, and owned window events. Keep application save/close policies in managed handlers. Programmatic state updates do not automatically reproduce user events.

List/table/tree DataView models use stable row IDs, columns, selection, expansion, sorting, and owned cell values. Dataset limits are 100000 rows, 32 columns, 16 MiB UTF-8 content, and tree depth 64. Generation yields every 256 rows and commits a completed model; failed or stopped updates discard uncommitted state. Serialize competing async operations on a view. These are bounded copied models, not remote paging or unbounded lists.

## Drawing and GPU resources

Metal APIs cover devices, buffers, textures, samplers, pipelines, compute/render commands, offscreen targets, uploads/readback, and asynchronous GPU completion. Resources must remain live through submitted work. Await physical completion before reuse or destroy.

Batch2D preserves draw order and batches compatible texture/filter/clip operations. Low-level coordinates are top-left drawable pixels. The managed canvas uses points with backing scale, and offers explicit pixel mode. Images are decoded/uploaded on Main. Text2D is a reusable shaped-text/texture snapshot; rebuild it when text changes.

Canvas jobs manage a surface, batch, frame waits, and cleanup. Invalidation coalesces requests. Idle/hidden/minimized/occluded views stop their frame timer; unavailable drawables use bounded backoff. FrameClock schedules Main RunLoop deadlines and does not promise display-link synchronization or exact FPS.

## Documents and sheets

The document layer provides state, selection, undo/redo, snapshots, and explicit file saving. Await saves and resource users before closing. Open/save panels and alert sheets complete asynchronously; distinguish user cancellation from ordinary Error and runtime cancellation. Keep UI event and text ownership rules when composing IME or document callbacks.

## Bundled resources

session.resourceRoot returns Option<String> for application assets, or None in an ordinary JIT/console host. resourcePath validates a package-relative name and returns its bundled absolute path without reading it or changing cwd.

Use rinzu bundle to create an application from macos-app.json, then bundle-verify to inspect the generated inventory and ad-hoc signature. Keep the entire bundle together. Native OS-image compatibility still applies. Developer ID signing, notarization, and App Store submission are separate distribution steps.

See [AOT and bundle distribution](../../docs/tooling/distribution.md).
