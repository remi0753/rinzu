# macOS applications and GUI

`rinzu.macos` is an official SDK for AppKit and Metal on macOS arm64. It requires a desktop session. Import it directly with a matching toolchain bundle. When working from a source checkout, you can use an explicit local package dependency instead:

```toml
[dependencies]
"rinzu.macos" = { path = "../rinzu/sdk/macos" }
```

The public API covers Session, Window, View, widgets, owned events, async application handlers, Metal resources, 2D drawing, documents, and sheets. [The SDK reference](../../sdk/macos/README.md) describes the application entry and resource types; [the source](../../sdk/macos/src/macos.rinzu) contains exact signatures.

## A minimal window

Create an application package and add this `src/main.rinzu`. It displays a label and requests a clean stop when the user closes the window or quits.

```rinzu
import rinzu.macos;

async fn screen(app: rinzu.macos.UiApplication) {
    let scope = app.primary;
    let label = scope.label("Hello, Rinzu!", rinzu.macos.Rect(x: 20.0, y: 20.0, width: 300.0, height: 30.0));
    let root = scope.window.contentView();
    let added = root.addSubview(label.view);
    await scope.dispatch(
        async fn (owner: rinzu.macos.UiScope, event: rinzu.macos.UiEvent) {
            if event.kind == rinzu.macos.eventCloseRequested || event.kind == rinzu.macos.eventQuitRequested {
                owner.requestStop();
            }
        }
    );
}

async fn main() {
    try {
        let status = await rinzu.macos.applicationWithSignals("My App", 480.0, 320.0, screen);
    }
    catch problems {
        panic("application failed");
    }
}
```

```sh
rinzu check ./my-app
rinzu run ./my-app
```

This example needs a desktop session when run; checking it does not open a window.

## Application ownership

The application session and its windows run on physical Main. Use the high-level application scope to associate UI resources and managed jobs with the application lifecycle. Stop event intake, await active handlers and GPU/native completion, then release resources. Background tasks cannot receive UI handles through Worker transfer.

The low-level run handler is synchronous and receives a RunContext containing a session. Native code retains that context through the actual call and callback settlement. Build asynchronous application behavior using the SDK's managed handler layer rather than storing arbitrary managed closures in native code.

## Choose an API layer

| Feature | Public concepts |
| --- | --- |
| Native controls | Window, View, buttons, text controls, layout, accessibility, themes |
| Data views | List/table/tree DataView, stable row IDs, columns, selection and sort events |
| Text editing | Document state, undo/redo, selection, snapshots and file saving |
| Sheets | Open/save panels and alerts with asynchronous completion |
| GPU | Device, buffers, textures, samplers, pipelines, command submission and waits |
| 2D drawing | Batch2D, images, shaped text, canvas jobs, frame clocks |

Data models are bounded: datasets have at most 100000 rows, 32 columns, and 16 MiB of UTF-8 cell data; trees have at most 64 levels. Updates yield during generation and commit the complete model. Serialize competing async updates to one view in application code.

Batch2D preserves draw order while batching compatible texture/filter/clip state. Low-level batch coordinates use drawable pixels; the high-level canvas uses points and backing scale. A FrameClock schedules Main RunLoop deadlines; it does not promise display-link synchronization or exact frame rates.

## Resource access and application bundles

Create `macos-app.json` in an application package:

```json
{
  "schema": "rinzu.macos-app-v1",
  "identifier": "dev.example.myapp",
  "name": "My App",
  "version": "0.1.0",
  "build": "1",
  "resources": ["assets"]
}
```

```sh
rinzu bundle ./my-app --output './My App.app'
rinzu bundle-verify './My App.app'
```

Bundle creation does not execute the application's Main. Resources use package-relative paths and cannot be symlinks. The optional icon accepts a 1024×1024 PNG or a supported ICNS file.

`session.resourceRoot()` returns Option<String> for bundled application resources and None in an ordinary JIT/console host. `session.resourcePath("assets/title.txt")` validates a relative resource name and returns its absolute bundled path; it does not read the file or change cwd. Select a development path when running without a bundle.

See [Distribution](../tooling/distribution.md) for sidecars, signatures, relocation, and native OS compatibility requirements.
