# rinzu.macos API reference

Generated from checked public exports and captured source comments.

Language: `rinzu-0.5` · target: `aarch64-apple-darwin` · semantics: `rinzu-language-0.5-r85`.

## rinzu.macos.Batch2D

class · [source](../../sdk/macos/src/macos.rinzu#L3209).

```rinzu
class Batch2D
```

Bounded, reusable quad batch. Coordinates and clip use target pixels, top-left.
Reset only after the command using its vertex buffer has physically settled.

### field vertices

[source](../../sdk/macos/src/macos.rinzu#L3210).

```rinzu
const vertices: Buffer
```

### field rgba

[source](../../sdk/macos/src/macos.rinzu#L3211).

```rinzu
const rgba: RenderPipeline
```

### field bgra

[source](../../sdk/macos/src/macos.rinzu#L3212).

```rinzu
const bgra: RenderPipeline
```

### field nearest

[source](../../sdk/macos/src/macos.rinzu#L3213).

```rinzu
const nearest: Sampler
```

### field linear

[source](../../sdk/macos/src/macos.rinzu#L3214).

```rinzu
const linear: Sampler
```

### field white

[source](../../sdk/macos/src/macos.rinzu#L3215).

```rinzu
const white: Texture
```

### field command

[source](../../sdk/macos/src/macos.rinzu#L3216).

```rinzu
const command: Command
```

### field capacity

[source](../../sdk/macos/src/macos.rinzu#L3217).

```rinzu
const capacity: Int
```

### field width

[source](../../sdk/macos/src/macos.rinzu#L3218).

```rinzu
width: Int
```

### field height

[source](../../sdk/macos/src/macos.rinzu#L3219).

```rinzu
height: Int
```

### field count

[source](../../sdk/macos/src/macos.rinzu#L3220).

```rinzu
count: Int
```

### field draws

[source](../../sdk/macos/src/macos.rinzu#L3221).

```rinzu
draws: List<BatchDraw2D>
```

### field transform

[source](../../sdk/macos/src/macos.rinzu#L3222).

```rinzu
transform: Transform2D
```

### field clip

[source](../../sdk/macos/src/macos.rinzu#L3223).

```rinzu
clip: Scissor
```

### field filter

[source](../../sdk/macos/src/macos.rinzu#L3224).

```rinzu
filter: Int
```

### field closed

[source](../../sdk/macos/src/macos.rinzu#L3225).

```rinzu
closed: Bool
```

### field rendering

[source](../../sdk/macos/src/macos.rinzu#L3226).

```rinzu
rendering: Bool
```

## rinzu.macos.Batch2D.destroy

Checked type: `fn(Batch2D)->Unit`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L3477).

```rinzu
fn destroy(self: Batch2D)
```

## rinzu.macos.Batch2D.drawCallCount

Checked type: `fn(Batch2D)->Int`.

Error contract: `nothrow` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L3368).

```rinzu
fn drawCallCount(self: Batch2D) -> Int
```

## rinzu.macos.Batch2D.encode

Checked type: `fn(Batch2D,Command,Int)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L3373).

```rinzu
fn encode(self: Batch2D, command: Command, format: Int) -> Int
```

Encode into an already begun pass. On Error the caller must abandon it.

## rinzu.macos.Batch2D.fillRect

Checked type: `fn(Batch2D,Rect,Color)->Unit`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L3285).

```rinzu
fn fillRect(self: Batch2D, destination: Rect, color: Color)
```

## rinzu.macos.Batch2D.image

Checked type: `fn(Batch2D,Texture,Rect,Color)->Unit`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L3303).

```rinzu
fn image(self: Batch2D, texture: Texture, destination: Rect, tint: Color)
```

## rinzu.macos.Batch2D.imageRegion

Checked type: `fn(Batch2D,Texture,Rect,Rect,Color)->Unit`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L3308).

```rinzu
fn imageRegion(self: Batch2D, texture: Texture, destination: Rect, uv: Rect, tint: Color)
```

Normalized UVs, with row zero at the top. Texture pixels must be premultiplied.

## rinzu.macos.Batch2D.render

Checked type: `async(Batch2D,View,Color)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L3410).

```rinzu
async fn render(self: Batch2D, surface: View, clear: Color) -> Int
```

Own a complete frame, including abandon / physical drain on Error or cancel.

## rinzu.macos.Batch2D.renderTexture

Checked type: `async(Batch2D,Texture,Color)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L3443).

```rinzu
async fn renderTexture(self: Batch2D, texture: Texture, clear: Color) -> Int
```

## rinzu.macos.Batch2D.reset

Checked type: `fn(Batch2D,Int,Int)->Unit`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L3228).

```rinzu
fn reset(self: Batch2D, width: Int, height: Int)
```

## rinzu.macos.Batch2D.setClip

Checked type: `fn(Batch2D,Scissor)->Unit`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L3256).

```rinzu
fn setClip(self: Batch2D, clip: Scissor)
```

## rinzu.macos.Batch2D.setFilter

Checked type: `fn(Batch2D,Int)->Unit`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L3274).

```rinzu
fn setFilter(self: Batch2D, filter: Int)
```

## rinzu.macos.Batch2D.setTransform

Checked type: `fn(Batch2D,Transform2D)->Unit`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L3248).

```rinzu
fn setTransform(self: Batch2D, transform: Transform2D)
```

## rinzu.macos.Batch2D.text

Checked type: `fn(Batch2D,Text2D,Float,Float,Color)->Unit`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L3290).

```rinzu
fn text(self: Batch2D, text: Text2D, x: Float, y: Float, tint: Color)
```

Coordinates use the current batch transform. TextMetrics uses font points.

## rinzu.macos.BatchDraw2D

class · [source](../../sdk/macos/src/macos.rinzu#L3197).

```rinzu
class BatchDraw2D
```

A contiguous draw span. Constructed by Batch2D; do not mutate batch storage.

### field texture

[source](../../sdk/macos/src/macos.rinzu#L3198).

```rinzu
const texture: Texture
```

### field textureId

[source](../../sdk/macos/src/macos.rinzu#L3199).

```rinzu
const textureId: Int
```

### field sampler

[source](../../sdk/macos/src/macos.rinzu#L3200).

```rinzu
const sampler: Sampler
```

### field filter

[source](../../sdk/macos/src/macos.rinzu#L3201).

```rinzu
const filter: Int
```

### field clip

[source](../../sdk/macos/src/macos.rinzu#L3202).

```rinzu
const clip: Scissor
```

### field first

[source](../../sdk/macos/src/macos.rinzu#L3203).

```rinzu
const first: Int
```

### field count

[source](../../sdk/macos/src/macos.rinzu#L3204).

```rinzu
count: Int
```

## rinzu.macos.Buffer

class · [source](../../sdk/macos/src/macos.rinzu#L12).

```rinzu
class Buffer
```

### field handle

[source](../../sdk/macos/src/macos.rinzu#L13).

```rinzu
const handle: BufferHandle
```

## rinzu.macos.Buffer.destroy

Checked type: `fn(Buffer)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L27).

```rinzu
fn destroy(self: Buffer) -> Int
```

## rinzu.macos.Buffer.read

Checked type: `fn(Buffer,Int,Int)->ByteArray`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L22).

```rinzu
fn read(self: Buffer, offset: Int, length: Int) -> ByteArray
```

## rinzu.macos.Buffer.write

Checked type: `fn(Buffer,Int,ByteArray)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L15).

```rinzu
fn write(self: Buffer, offset: Int, bytes: ByteArray) -> Int
```

## rinzu.macos.Buffer.writeQuad2D

Checked type: `fn(Buffer,Int,Quad2D)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L35).

```rinzu
fn writeQuad2D(self: Buffer, index: Int, quad: Quad2D) -> Int
```

## rinzu.macos.Buffer.writeVertex2D

Checked type: `fn(Buffer,Int,Vertex2D)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L31).

```rinzu
fn writeVertex2D(self: Buffer, index: Int, vertex: Vertex2D) -> Int
```

## rinzu.macos.BufferHandle

Checked type: `BufferHandle`.

opaque · [source](../../sdk/macos/src/macos.rinzu#L984).

```rinzu
opaque BufferHandle;
```

## rinzu.macos.Button

class · [source](../../sdk/macos/src/macos.rinzu#L2580).

```rinzu
class Button
```

### field view

[source](../../sdk/macos/src/macos.rinzu#L2581).

```rinzu
const view: View
```

## rinzu.macos.Button.destroy

Checked type: `fn(Button)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2587).

```rinzu
fn destroy(self: Button) -> Int
```

## rinzu.macos.Button.identity

Checked type: `fn(Button)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2583).

```rinzu
fn identity(self: Button) -> Int
```

## rinzu.macos.Button.setEnabled

Checked type: `fn(Button,Bool)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2591).

```rinzu
fn setEnabled(self: Button, enabled: Bool) -> Int
```

## rinzu.macos.Button.setText

Checked type: `fn(Button,String)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2599).

```rinzu
fn setText(self: Button, value: String) -> Int
```

## rinzu.macos.Button.text

Checked type: `fn(Button)->String`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2595).

```rinzu
fn text(self: Button) -> String
```

## rinzu.macos.Canvas2D

class · [source](../../sdk/macos/src/macos.rinzu#L3559).

```rinzu
class Canvas2D
```

### field view

[source](../../sdk/macos/src/macos.rinzu#L3560).

```rinzu
const view: View
```

### field clock

[source](../../sdk/macos/src/macos.rinzu#L3561).

```rinzu
const clock: FrameClock
```

### field batch

[source](../../sdk/macos/src/macos.rinzu#L3562).

```rinzu
const batch: Batch2D
```

### field clear

[source](../../sdk/macos/src/macos.rinzu#L3563).

```rinzu
clear: Color
```

### field frames

[source](../../sdk/macos/src/macos.rinzu#L3564).

```rinzu
frames: Int
```

### field drawing

[source](../../sdk/macos/src/macos.rinzu#L3565).

```rinzu
drawing: Bool
```

### field stopped

[source](../../sdk/macos/src/macos.rinzu#L3566).

```rinzu
stopped: Bool
```

### field disposed

[source](../../sdk/macos/src/macos.rinzu#L3567).

```rinzu
disposed: Bool
```

## rinzu.macos.Canvas2D.invalidate

Checked type: `fn(Canvas2D)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L3569).

```rinzu
fn invalidate(self: Canvas2D) -> Int
```

## rinzu.macos.Canvas2D.setAnimating

Checked type: `fn(Canvas2D,Bool)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L3576).

```rinzu
fn setAnimating(self: Canvas2D, enabled: Bool) -> Int
```

## rinzu.macos.Canvas2D.setClear

Checked type: `fn(Canvas2D,Color)->Unit`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L3583).

```rinzu
fn setClear(self: Canvas2D, color: Color)
```

## rinzu.macos.Canvas2D.stop

Checked type: `fn(Canvas2D)->Unit`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L3588).

```rinzu
fn stop(self: Canvas2D)
```

## rinzu.macos.Checkbox

class · [source](../../sdk/macos/src/macos.rinzu#L2606).

```rinzu
class Checkbox
```

### field view

[source](../../sdk/macos/src/macos.rinzu#L2607).

```rinzu
const view: View
```

## rinzu.macos.Checkbox.checked

Checked type: `fn(Checkbox)->Bool`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2629).

```rinzu
fn checked(self: Checkbox) -> Bool
```

## rinzu.macos.Checkbox.destroy

Checked type: `fn(Checkbox)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2613).

```rinzu
fn destroy(self: Checkbox) -> Int
```

## rinzu.macos.Checkbox.identity

Checked type: `fn(Checkbox)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2609).

```rinzu
fn identity(self: Checkbox) -> Int
```

## rinzu.macos.Checkbox.setChecked

Checked type: `fn(Checkbox,Bool)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2634).

```rinzu
fn setChecked(self: Checkbox, value: Bool) -> Int
```

## rinzu.macos.Checkbox.setEnabled

Checked type: `fn(Checkbox,Bool)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2617).

```rinzu
fn setEnabled(self: Checkbox, enabled: Bool) -> Int
```

## rinzu.macos.Checkbox.setText

Checked type: `fn(Checkbox,String)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2625).

```rinzu
fn setText(self: Checkbox, value: String) -> Int
```

## rinzu.macos.Checkbox.text

Checked type: `fn(Checkbox)->String`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2621).

```rinzu
fn text(self: Checkbox) -> String
```

## rinzu.macos.Color

Checked type: `Color`.

native-record · [source](../../sdk/macos/src/macos.rinzu#L1015).

```rinzu
record Color
```

### field red

[source](../../sdk/macos/src/macos.rinzu#L1016).

```rinzu
red: Float
```

### field green

[source](../../sdk/macos/src/macos.rinzu#L1017).

```rinzu
green: Float
```

### field blue

[source](../../sdk/macos/src/macos.rinzu#L1018).

```rinzu
blue: Float
```

### field alpha

[source](../../sdk/macos/src/macos.rinzu#L1019).

```rinzu
alpha: Float
```

## rinzu.macos.Command

class · [source](../../sdk/macos/src/macos.rinzu#L60).

```rinzu
class Command
```

### field handle

[source](../../sdk/macos/src/macos.rinzu#L61).

```rinzu
const handle: CommandHandle
```

## rinzu.macos.Command.abandon

Checked type: `fn(Command)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L108).

```rinzu
fn abandon(self: Command) -> Int
```

## rinzu.macos.Command.beginCompute

Checked type: `fn(Command,ComputePipeline)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L63).

```rinzu
fn beginCompute(self: Command, pipeline: ComputePipeline) -> Int
```

## rinzu.macos.Command.beginRender

Checked type: `fn(Command,View,Color)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L67).

```rinzu
fn beginRender(self: Command, view: View, clear: Color) -> Int
```

## rinzu.macos.Command.beginRenderTexture

Checked type: `fn(Command,Texture,Color)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L120).

```rinzu
fn beginRenderTexture(self: Command, texture: Texture, clear: Color) -> Int
```

## rinzu.macos.Command.commit

Checked type: `fn(Command)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L104).

```rinzu
fn commit(self: Command) -> Int
```

## rinzu.macos.Command.destroy

Checked type: `fn(Command)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L116).

```rinzu
fn destroy(self: Command) -> Int
```

## rinzu.macos.Command.dispatch

Checked type: `fn(Command,Int,Int,Int,Int,Int,Int)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L96).

```rinzu
fn dispatch(self: Command, x: Int, y: Int, z: Int, tx: Int, ty: Int, tz: Int) -> Int
```

## rinzu.macos.Command.drawTriangles

Checked type: `fn(Command,Int,Int)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L100).

```rinzu
fn drawTriangles(self: Command, start: Int, count: Int) -> Int
```

## rinzu.macos.Command.setComputeBuffer

Checked type: `fn(Command,Int,Buffer,Int)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L75).

```rinzu
fn setComputeBuffer(self: Command, index: Int, buffer: Buffer, offset: Int) -> Int
```

## rinzu.macos.Command.setComputeSampler

Checked type: `fn(Command,Int,Sampler)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L148).

```rinzu
fn setComputeSampler(self: Command, index: Int, sampler: Sampler) -> Int
```

## rinzu.macos.Command.setComputeTexture

Checked type: `fn(Command,Int,Texture)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L133).

```rinzu
fn setComputeTexture(self: Command, index: Int, texture: Texture) -> Int
```

## rinzu.macos.Command.setFragmentBuffer

Checked type: `fn(Command,Int,Buffer,Int)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L89).

```rinzu
fn setFragmentBuffer(self: Command, index: Int, buffer: Buffer, offset: Int) -> Int
```

## rinzu.macos.Command.setFragmentSampler

Checked type: `fn(Command,Int,Sampler)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L156).

```rinzu
fn setFragmentSampler(self: Command, index: Int, sampler: Sampler) -> Int
```

## rinzu.macos.Command.setFragmentTexture

Checked type: `fn(Command,Int,Texture)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L141).

```rinzu
fn setFragmentTexture(self: Command, index: Int, texture: Texture) -> Int
```

## rinzu.macos.Command.setRenderPipeline

Checked type: `fn(Command,RenderPipeline)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L71).

```rinzu
fn setRenderPipeline(self: Command, pipeline: RenderPipeline) -> Int
```

## rinzu.macos.Command.setScissor

Checked type: `fn(Command,Scissor)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L129).

```rinzu
fn setScissor(self: Command, scissor: Scissor) -> Int
```

## rinzu.macos.Command.setVertexBuffer

Checked type: `fn(Command,Int,Buffer,Int)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L82).

```rinzu
fn setVertexBuffer(self: Command, index: Int, buffer: Buffer, offset: Int) -> Int
```

## rinzu.macos.Command.setVertexSampler

Checked type: `fn(Command,Int,Sampler)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L152).

```rinzu
fn setVertexSampler(self: Command, index: Int, sampler: Sampler) -> Int
```

## rinzu.macos.Command.setVertexTexture

Checked type: `fn(Command,Int,Texture)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L137).

```rinzu
fn setVertexTexture(self: Command, index: Int, texture: Texture) -> Int
```

## rinzu.macos.Command.setViewport

Checked type: `fn(Command,Rect)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L125).

```rinzu
fn setViewport(self: Command, viewport: Rect) -> Int
```

Render state uses pixels with top-left origin, independently of AppKit points.

## rinzu.macos.Command.wait

Checked type: `async(Command)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L112).

```rinzu
async fn wait(self: Command) -> Int
```

## rinzu.macos.CommandHandle

Checked type: `CommandHandle`.

opaque · [source](../../sdk/macos/src/macos.rinzu#L1002).

```rinzu
opaque CommandHandle;
```

## rinzu.macos.ComputePipeline

class · [source](../../sdk/macos/src/macos.rinzu#L164).

```rinzu
class ComputePipeline
```

### field handle

[source](../../sdk/macos/src/macos.rinzu#L165).

```rinzu
const handle: ComputePipelineHandle
```

## rinzu.macos.ComputePipeline.destroy

Checked type: `fn(ComputePipeline)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L167).

```rinzu
fn destroy(self: ComputePipeline) -> Int
```

## rinzu.macos.ComputePipelineHandle

Checked type: `ComputePipelineHandle`.

opaque · [source](../../sdk/macos/src/macos.rinzu#L998).

```rinzu
opaque ComputePipelineHandle;
```

## rinzu.macos.DataColumn

class · [source](../../sdk/macos/src/macos.rinzu#L3918).

```rinzu
class DataColumn
```

### field title

[source](../../sdk/macos/src/macos.rinzu#L3919).

```rinzu
const title: String
```

### field width

[source](../../sdk/macos/src/macos.rinzu#L3920).

```rinzu
const width: Float
```

### field sortable

[source](../../sdk/macos/src/macos.rinzu#L3921).

```rinzu
const sortable: Bool
```

## rinzu.macos.DataRow

class · [source](../../sdk/macos/src/macos.rinzu#L3925).

```rinzu
class DataRow
```

IDs are positive and unique. A tree parent must precede its children; zero is a root.

### field id

[source](../../sdk/macos/src/macos.rinzu#L3926).

```rinzu
const id: Int
```

### field parent

[source](../../sdk/macos/src/macos.rinzu#L3927).

```rinzu
const parent: Int
```

### field cells

[source](../../sdk/macos/src/macos.rinzu#L3928).

```rinzu
const cells: List<String>
```

## rinzu.macos.DataState

Checked type: `DataState`.

native-record · [source](../../sdk/macos/src/macos.rinzu#L3857).

```rinzu
record DataState
```

### field rows

[source](../../sdk/macos/src/macos.rinzu#L3858).

```rinzu
rows: Int
```

### field visibleRows

[source](../../sdk/macos/src/macos.rinzu#L3859).

```rinzu
visibleRows: Int
```

### field cellViews

[source](../../sdk/macos/src/macos.rinzu#L3860).

```rinzu
cellViews: Int
```

### field bytes

[source](../../sdk/macos/src/macos.rinzu#L3861).

```rinzu
bytes: Int
```

### field revision

[source](../../sdk/macos/src/macos.rinzu#L3862).

```rinzu
revision: Int
```

### field selectedId

[source](../../sdk/macos/src/macos.rinzu#L3863).

```rinzu
selectedId: Int
```

### field selectedRow

[source](../../sdk/macos/src/macos.rinzu#L3864).

```rinzu
selectedRow: Int
```

### field firstVisible

[source](../../sdk/macos/src/macos.rinzu#L3865).

```rinzu
firstVisible: Int
```

### field lastVisible

[source](../../sdk/macos/src/macos.rinzu#L3866).

```rinzu
lastVisible: Int
```

### field pendingRows

[source](../../sdk/macos/src/macos.rinzu#L3867).

```rinzu
pendingRows: Int
```

## rinzu.macos.DataView

class · [source](../../sdk/macos/src/macos.rinzu#L3932).

```rinzu
class DataView
```

Shared List/Table/Tree interface. Model and AppKit datasource contain native copies only.

### field view

[source](../../sdk/macos/src/macos.rinzu#L3933).

```rinzu
const view: View
```

### field columns

[source](../../sdk/macos/src/macos.rinzu#L3934).

```rinzu
const columns: Int
```

### field control

[source](../../sdk/macos/src/macos.rinzu#L3935).

```rinzu
const control: UiControl
```

## rinzu.macos.DataView.accepts

Checked type: `fn(DataView,UiEvent)->Bool`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L3962).

```rinzu
fn accepts(self: DataView, event: UiEvent) -> Bool
```

## rinzu.macos.DataView.cell

Checked type: `fn(DataView,Int,Int)->String`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L3990).

```rinzu
fn cell(self: DataView, id: Int, column: Int) -> String
```

## rinzu.macos.DataView.destroy

Checked type: `fn(DataView)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L3941).

```rinzu
fn destroy(self: DataView) -> Int
```

## rinzu.macos.DataView.generateRows

Checked type: `async(DataView,Int,$rinzu_closure_530a103bc12515a3)->Unit`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L4004).

```rinzu
async fn generateRows(self: DataView, count: Int, make: fn (Int) -> DataRow)
```

Build offscreen, yield every 256 rows, and publish one complete model.

## rinzu.macos.DataView.identity

Checked type: `fn(DataView)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L3937).

```rinzu
fn identity(self: DataView) -> Int
```

## rinzu.macos.DataView.isExpanded

Checked type: `fn(DataView,Int)->Bool`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L3982).

```rinzu
fn isExpanded(self: DataView, id: Int) -> Bool
```

## rinzu.macos.DataView.scrollTo

Checked type: `fn(DataView,Int)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L3974).

```rinzu
fn scrollTo(self: DataView, id: Int) -> Int
```

## rinzu.macos.DataView.select

Checked type: `fn(DataView,Int,Bool)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L3970).

```rinzu
fn select(self: DataView, id: Int, reveal: Bool) -> Int
```

## rinzu.macos.DataView.setExpanded

Checked type: `fn(DataView,Int,Bool)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L3978).

```rinzu
fn setExpanded(self: DataView, id: Int, expanded: Bool) -> Int
```

## rinzu.macos.DataView.setRows

Checked type: `async(DataView,List<DataRow>)->Unit`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L4035).

```rinzu
async fn setRows(self: DataView, rows: List<DataRow>)
```

## rinzu.macos.DataView.sort

Checked type: `fn(DataView,Int,Bool)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L3986).

```rinzu
fn sort(self: DataView, column: Int, ascending: Bool) -> Int
```

## rinzu.macos.DataView.state

Checked type: `fn(DataView)->DataState`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L3945).

```rinzu
fn state(self: DataView) -> DataState
```

## rinzu.macos.Device

class · [source](../../sdk/macos/src/macos.rinzu#L172).

```rinzu
class Device
```

### field handle

[source](../../sdk/macos/src/macos.rinzu#L173).

```rinzu
const handle: DeviceHandle
```

## rinzu.macos.Device.allocateBuffer

Checked type: `fn(Device,Int)->Buffer`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L181).

```rinzu
fn allocateBuffer(self: Device, length: Int) -> Buffer
```

## rinzu.macos.Device.batch2D

Checked type: `fn(Device,Int)->Batch2D`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L245).

```rinzu
fn batch2D(self: Device, capacity: Int) -> Batch2D
```

## rinzu.macos.Device.buffer

Checked type: `fn(Device,ByteArray)->Buffer`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L175).

```rinzu
fn buffer(self: Device, bytes: ByteArray) -> Buffer
```

## rinzu.macos.Device.command

Checked type: `fn(Device)->Command`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L208).

```rinzu
fn command(self: Device) -> Command
```

Allocate a reusable command handle, normally once per in-flight frame slot.

## rinzu.macos.Device.destroy

Checked type: `fn(Device)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L212).

```rinzu
fn destroy(self: Device) -> Int
```

## rinzu.macos.Device.imageTexture

Checked type: `fn(Device,ByteArray)->Texture`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L225).

```rinzu
fn imageTexture(self: Device, encoded: ByteArray) -> Texture
```

Decode frame zero, normalize EXIF orientation and premultiplied sRGB RGBA8.
Encoded input <=64 MiB, dimensions <=8192, decoded pixels <=256 MiB.

## rinzu.macos.Device.library

Checked type: `fn(Device,String)->Library`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L185).

```rinzu
fn library(self: Device, source: String) -> Library
```

## rinzu.macos.Device.loadImageTexture

Checked type: `async(Device,String)->Texture`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L295).

```rinzu
async fn loadImageTexture(self: Device, path: String) -> Texture
```

Stream the encoded file with a 64 MiB limit, then decode on Main.
Decode/upload is synchronous; cooperative async decode is a later extension.

## rinzu.macos.Device.loadLibrary

Checked type: `fn(Device,String)->Library`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L194).

```rinzu
fn loadLibrary(self: Device, path: String) -> Library
```

## rinzu.macos.Device.rasterText

Checked type: `fn(Device,TextLayout,Float)->Texture`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L300).

```rinzu
fn rasterText(self: Device, layout: TextLayout, scale: Float) -> Texture
```

## rinzu.macos.Device.sampler

Checked type: `fn(Device,Int,Int)->Sampler`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L233).

```rinzu
fn sampler(self: Device, filter: Int, address: Int) -> Sampler
```

## rinzu.macos.Device.surface

Checked type: `fn(Device,Rect)->View`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L203).

```rinzu
fn surface(self: Device, rect: Rect) -> View
```

## rinzu.macos.Device.text2D

Checked type: `fn(Device,Font,String,Float,Int,Float)->Text2D`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L306).

```rinzu
fn text2D(self: Device, font: Font, text: String, width: Float, alignment: Int, scale: Float) -> Text2D
```

## rinzu.macos.Device.texture2D

Checked type: `fn(Device,Int,Int,Int,Int)->Texture`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L217).

```rinzu
fn texture2D(self: Device, width: Int, height: Int, format: Int, usage: Int) -> Texture
```

Single-level RGBA8 / BGRA8 unorm texture. Usage is a sum of distinct flags.

## rinzu.macos.Device.vertexBuffer2D

Checked type: `fn(Device,Int)->Buffer`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L238).

```rinzu
fn vertexBuffer2D(self: Device, capacity: Int) -> Buffer
```

Allocate packed float32 x,y,u,v,r,g,b,a vertices (32 bytes each).

## rinzu.macos.DeviceHandle

Checked type: `DeviceHandle`.

opaque · [source](../../sdk/macos/src/macos.rinzu#L982).

```rinzu
opaque DeviceHandle;
```

## rinzu.macos.Document

class · [source](../../sdk/macos/src/document.rinzu#L117).

```rinzu
class Document
```

Source document state. Native input updates this model without echoing setText.
Public mutations finish synchronously under cancellation masking.

### field undo

[source](../../sdk/macos/src/document.rinzu#L118).

```rinzu
const undo: DocumentHistory
```

### field redo

[source](../../sdk/macos/src/document.rinzu#L119).

```rinzu
const redo: DocumentHistory
```

### field text

[source](../../sdk/macos/src/document.rinzu#L120).

```rinzu
text: String
```

### field start

[source](../../sdk/macos/src/document.rinzu#L121).

```rinzu
start: Int
```

### field end

[source](../../sdk/macos/src/document.rinzu#L122).

```rinzu
end: Int
```

### field version

[source](../../sdk/macos/src/document.rinzu#L123).

```rinzu
version: Int
```

### field savedVersion

[source](../../sdk/macos/src/document.rinzu#L124).

```rinzu
savedVersion: Int
```

### field accepting

[source](../../sdk/macos/src/document.rinzu#L125).

```rinzu
accepting: Bool
```

### field edits

[source](../../sdk/macos/src/document.rinzu#L126).

```rinzu
edits: Int
```

## rinzu.macos.Document.accepts

Checked type: `fn(document::Document,document::DocumentSnapshot)->Bool`.

Error contract: `nothrow` (inferred).

method · [source](../../sdk/macos/src/document.rinzu#L288).

```rinzu
fn accepts(self: Document, snapshot: DocumentSnapshot) -> Bool
```

## rinzu.macos.Document.create

Checked type: `fn(String,Bool)->document::Document`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/document.rinzu#L128).

```rinzu
fn create(text: String, saved: Bool) -> Document
```

## rinzu.macos.Document.dirty

Checked type: `fn(document::Document)->Bool`.

Error contract: `nothrow` (inferred).

method · [source](../../sdk/macos/src/document.rinzu#L178).

```rinzu
fn dirty(self: Document) -> Bool
```

## rinzu.macos.Document.edit

Checked type: `fn(document::Document,String,Int,Int)->Bool`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/document.rinzu#L197).

```rinzu
fn edit(self: Document, text: String, start: Int, end: Int) -> Bool
```

## rinzu.macos.Document.markSaved

Checked type: `fn(document::Document,document::DocumentSnapshot)->Unit`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/document.rinzu#L281).

```rinzu
fn markSaved(self: Document, snapshot: DocumentSnapshot)
```

## rinzu.macos.Document.redoEdit

Checked type: `fn(document::Document)->Bool`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/document.rinzu#L253).

```rinzu
fn redoEdit(self: Document) -> Bool
```

## rinzu.macos.Document.reset

Checked type: `fn(document::Document,String,Bool)->Bool`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/document.rinzu#L156).

```rinzu
fn reset(self: Document, text: String, saved: Bool) -> Bool
```

Install another document after its read succeeds; invalidate outstanding snapshots.

## rinzu.macos.Document.select

Checked type: `fn(document::Document,Int,Int)->Unit`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/document.rinzu#L182).

```rinzu
fn select(self: Document, start: Int, end: Int)
```

## rinzu.macos.Document.snapshot

Checked type: `fn(document::Document)->document::DocumentSnapshot`.

Error contract: `nothrow` (inferred).

method · [source](../../sdk/macos/src/document.rinzu#L151).

```rinzu
fn snapshot(self: Document) -> DocumentSnapshot
```

## rinzu.macos.Document.undoEdit

Checked type: `fn(document::Document)->Bool`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/document.rinzu#L225).

```rinzu
fn undoEdit(self: Document) -> Bool
```

## rinzu.macos.Document.withHistory

Checked type: `fn(String,Bool,Int,Int)->document::Document`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/document.rinzu#L132).

```rinzu
fn withHistory(text: String, saved: Bool, maximumEntries: Int, maximumBytes: Int) -> Document
```

## rinzu.macos.DocumentBinding

class · [source](../../sdk/macos/src/macos.rinzu#L2857).

```rinzu
class DocumentBinding
```

A Source document bound to an existing TextView. The Window owns the native View.

### field session

[source](../../sdk/macos/src/macos.rinzu#L2858).

```rinzu
const session: Session
```

### field input

[source](../../sdk/macos/src/macos.rinzu#L2859).

```rinzu
const input: TextView
```

### field document

[source](../../sdk/macos/src/macos.rinzu#L2860).

```rinzu
const document: documents.Document
```

### field barrier

[source](../../sdk/macos/src/macos.rinzu#L2861).

```rinzu
barrier: Int
```

## rinzu.macos.DocumentBinding.capture

Checked type: `fn(DocumentBinding)->Bool`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2874).

```rinzu
fn capture(self: DocumentBinding) -> Bool
```

## rinzu.macos.DocumentBinding.create

Checked type: `fn(Session,TextView,document::Document)->DocumentBinding`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2863).

```rinzu
fn create(session: Session, input: TextView, document: documents.Document) -> DocumentBinding
```

## rinzu.macos.DocumentBinding.load

Checked type: `fn(DocumentBinding,String)->Bool`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2886).

```rinzu
fn load(self: DocumentBinding, text: String) -> Bool
```

Replace the document after a successful load, with fresh history and version.

## rinzu.macos.DocumentBinding.receive

Checked type: `fn(DocumentBinding,UiEvent)->Bool`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2912).

```rinzu
fn receive(self: DocumentBinding, event: UiEvent) -> Bool
```

## rinzu.macos.DocumentBinding.redoEdit

Checked type: `fn(DocumentBinding)->Bool`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2983).

```rinzu
fn redoEdit(self: DocumentBinding) -> Bool
```

## rinzu.macos.DocumentBinding.replace

Checked type: `fn(DocumentBinding,String,Int,Int)->Bool`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2932).

```rinzu
fn replace(self: DocumentBinding, text: String, start: Int, end: Int) -> Bool
```

## rinzu.macos.DocumentBinding.setAccepting

Checked type: `fn(DocumentBinding,Bool)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L3008).

```rinzu
fn setAccepting(self: DocumentBinding, accepting: Bool) -> Int
```

## rinzu.macos.DocumentBinding.undoEdit

Checked type: `fn(DocumentBinding)->Bool`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2958).

```rinzu
fn undoEdit(self: DocumentBinding) -> Bool
```

## rinzu.macos.DocumentHistory

class · [source](../../sdk/macos/src/document.rinzu#L13).

```rinzu
class DocumentHistory
```

### field maximumEntries

[source](../../sdk/macos/src/document.rinzu#L14).

```rinzu
const maximumEntries: Int
```

### field maximumBytes

[source](../../sdk/macos/src/document.rinzu#L15).

```rinzu
const maximumBytes: Int
```

### field entries

[source](../../sdk/macos/src/document.rinzu#L16).

```rinzu
entries: List<DocumentSnapshot>
```

### field bytes

[source](../../sdk/macos/src/document.rinzu#L17).

```rinzu
bytes: Int
```

## rinzu.macos.DocumentHistory.clear

Checked type: `fn(document::DocumentHistory)->Unit`.

Error contract: `nothrow` (inferred).

method · [source](../../sdk/macos/src/document.rinzu#L26).

```rinzu
fn clear(self: DocumentHistory)
```

## rinzu.macos.DocumentHistory.create

Checked type: `fn(Int,Int)->document::DocumentHistory`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/document.rinzu#L19).

```rinzu
fn create(maximumEntries: Int, maximumBytes: Int) -> DocumentHistory
```

## rinzu.macos.DocumentHistory.length

Checked type: `fn(document::DocumentHistory)->Int`.

Error contract: `nothrow` (inferred).

method · [source](../../sdk/macos/src/document.rinzu#L38).

```rinzu
fn length(self: DocumentHistory) -> Int
```

## rinzu.macos.DocumentHistory.peek

Checked type: `fn(document::DocumentHistory)->Option<document::DocumentSnapshot>`.

Error contract: `nothrow` (inferred).

method · [source](../../sdk/macos/src/document.rinzu#L42).

```rinzu
fn peek(self: DocumentHistory) -> Option<DocumentSnapshot>
```

## rinzu.macos.DocumentHistory.pop

Checked type: `fn(document::DocumentHistory)->Option<document::DocumentSnapshot>`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/document.rinzu#L49).

```rinzu
fn pop(self: DocumentHistory) -> Option<DocumentSnapshot>
```

## rinzu.macos.DocumentHistory.push

Checked type: `fn(document::DocumentHistory,document::DocumentSnapshot)->Unit`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/document.rinzu#L78).

```rinzu
fn push(self: DocumentHistory, snapshot: DocumentSnapshot)
```

## rinzu.macos.DocumentSnapshot

class · [source](../../sdk/macos/src/document.rinzu#L6).

```rinzu
class DocumentSnapshot
```

Immutable, transferable input for saving and background analysis.

### field text

[source](../../sdk/macos/src/document.rinzu#L7).

```rinzu
const text: String
```

### field start

[source](../../sdk/macos/src/document.rinzu#L8).

```rinzu
const start: Int
```

### field end

[source](../../sdk/macos/src/document.rinzu#L9).

```rinzu
const end: Int
```

### field version

[source](../../sdk/macos/src/document.rinzu#L10).

```rinzu
const version: Int
```

## rinzu.macos.Event

Checked type: `Event`.

native-record · [source](../../sdk/macos/src/macos.rinzu#L1112).

```rinzu
record Event
```

### field kind

[source](../../sdk/macos/src/macos.rinzu#L1113).

```rinzu
kind: Int
```

### field windowId

[source](../../sdk/macos/src/macos.rinzu#L1114).

```rinzu
windowId: Int
```

### field viewId

[source](../../sdk/macos/src/macos.rinzu#L1115).

```rinzu
viewId: Int
```

### field sequence

[source](../../sdk/macos/src/macos.rinzu#L1116).

```rinzu
sequence: Int
```

### field x

[source](../../sdk/macos/src/macos.rinzu#L1117).

```rinzu
x: Float
```

### field y

[source](../../sdk/macos/src/macos.rinzu#L1118).

```rinzu
y: Float
```

### field width

[source](../../sdk/macos/src/macos.rinzu#L1119).

```rinzu
width: Float
```

### field height

[source](../../sdk/macos/src/macos.rinzu#L1120).

```rinzu
height: Float
```

### field scale

[source](../../sdk/macos/src/macos.rinzu#L1121).

```rinzu
scale: Float
```

### field keyCode

[source](../../sdk/macos/src/macos.rinzu#L1122).

```rinzu
keyCode: Int
```

### field modifiers

[source](../../sdk/macos/src/macos.rinzu#L1123).

```rinzu
modifiers: Int
```

### field action

[source](../../sdk/macos/src/macos.rinzu#L1124).

```rinzu
action: Int
```

### field selectionStart

[source](../../sdk/macos/src/macos.rinzu#L1125).

```rinzu
selectionStart: Int
```

### field selectionEnd

[source](../../sdk/macos/src/macos.rinzu#L1126).

```rinzu
selectionEnd: Int
```

### field flags

[source](../../sdk/macos/src/macos.rinzu#L1127).

```rinzu
flags: Int
```

### field value

[source](../../sdk/macos/src/macos.rinzu#L1128).

```rinzu
value: Float
```

### field deltaX

[source](../../sdk/macos/src/macos.rinzu#L1129).

```rinzu
deltaX: Float
```

### field deltaY

[source](../../sdk/macos/src/macos.rinzu#L1130).

```rinzu
deltaY: Float
```

## rinzu.macos.Font

class · [source](../../sdk/macos/src/macos.rinzu#L331).

```rinzu
class Font
```

### field handle

[source](../../sdk/macos/src/macos.rinzu#L332).

```rinzu
const handle: FontHandle
```

## rinzu.macos.Font.destroy

Checked type: `fn(Font)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L347).

```rinzu
fn destroy(self: Font) -> Int
```

## rinzu.macos.Font.layout

Checked type: `fn(Font,String,Float,Int)->TextLayout`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L335).

```rinzu
fn layout(self: Font, text: String, width: Float, alignment: Int) -> TextLayout
```

UTF8 <=64 KiB. Width zero is natural; positive width wraps. Alignment 0/1/2.

## rinzu.macos.FontHandle

Checked type: `FontHandle`.

opaque · [source](../../sdk/macos/src/macos.rinzu#L990).

```rinzu
opaque FontHandle;
```

## rinzu.macos.Frame2D

class · [source](../../sdk/macos/src/macos.rinzu#L3544).

```rinzu
class Frame2D
```

A frame begins in AppKit points / top-left; usePixels restores Batch2D pixel units.

### field info

[source](../../sdk/macos/src/macos.rinzu#L3545).

```rinzu
const info: FrameInfo
```

### field batch

[source](../../sdk/macos/src/macos.rinzu#L3546).

```rinzu
const batch: Batch2D
```

### field width

[source](../../sdk/macos/src/macos.rinzu#L3547).

```rinzu
const width: Float
```

### field height

[source](../../sdk/macos/src/macos.rinzu#L3548).

```rinzu
const height: Float
```

## rinzu.macos.Frame2D.usePixels

Checked type: `fn(Frame2D)->Unit`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L3550).

```rinzu
fn usePixels(self: Frame2D)
```

## rinzu.macos.Frame2D.usePoints

Checked type: `fn(Frame2D)->Unit`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L3554).

```rinzu
fn usePoints(self: Frame2D)
```

## rinzu.macos.FrameClock

class · [source](../../sdk/macos/src/macos.rinzu#L352).

```rinzu
class FrameClock
```

### field handle

[source](../../sdk/macos/src/macos.rinzu#L353).

```rinzu
const handle: FrameClockHandle
```

## rinzu.macos.FrameClock.destroy

Checked type: `fn(FrameClock)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L375).

```rinzu
fn destroy(self: FrameClock) -> Int
```

## rinzu.macos.FrameClock.invalidate

Checked type: `fn(FrameClock)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L355).

```rinzu
fn invalidate(self: FrameClock) -> Int
```

## rinzu.macos.FrameClock.next

Checked type: `async(FrameClock)->Option<FrameInfo>`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L395).

```rinzu
async fn next(self: FrameClock) -> Option<FrameInfo>
```

One reader. Cancellation disarms this wait and acknowledges physical publication.

## rinzu.macos.FrameClock.presented

Checked type: `fn(FrameClock)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L367).

```rinzu
fn presented(self: FrameClock) -> Int
```

## rinzu.macos.FrameClock.retry

Checked type: `fn(FrameClock)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L363).

```rinzu
fn retry(self: FrameClock) -> Int
```

## rinzu.macos.FrameClock.setAnimating

Checked type: `fn(FrameClock,Bool)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L359).

```rinzu
fn setAnimating(self: FrameClock, enabled: Bool) -> Int
```

## rinzu.macos.FrameClock.statistics

Checked type: `fn(FrameClock)->FrameStatistics`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L379).

```rinzu
fn statistics(self: FrameClock) -> FrameStatistics
```

## rinzu.macos.FrameClock.stop

Checked type: `fn(FrameClock)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L371).

```rinzu
fn stop(self: FrameClock) -> Int
```

## rinzu.macos.FrameClockHandle

Checked type: `FrameClockHandle`.

opaque · [source](../../sdk/macos/src/macos.rinzu#L994).

```rinzu
opaque FrameClockHandle;
```

## rinzu.macos.FrameInfo

Checked type: `FrameInfo`.

native-record · [source](../../sdk/macos/src/macos.rinzu#L1030).

```rinzu
record FrameInfo
```

### field sequence

[source](../../sdk/macos/src/macos.rinzu#L1031).

```rinzu
sequence: Int
```

### field width

[source](../../sdk/macos/src/macos.rinzu#L1032).

```rinzu
width: Int
```

### field height

[source](../../sdk/macos/src/macos.rinzu#L1033).

```rinzu
height: Int
```

### field scale

[source](../../sdk/macos/src/macos.rinzu#L1034).

```rinzu
scale: Float
```

### field time

[source](../../sdk/macos/src/macos.rinzu#L1035).

```rinzu
time: Float
```

### field delta

[source](../../sdk/macos/src/macos.rinzu#L1036).

```rinzu
delta: Float
```

## rinzu.macos.FrameStatistics

Checked type: `FrameStatistics`.

native-record · [source](../../sdk/macos/src/macos.rinzu#L1039).

```rinzu
record FrameStatistics
```

### field requests

[source](../../sdk/macos/src/macos.rinzu#L1040).

```rinzu
requests: Int
```

### field coalesced

[source](../../sdk/macos/src/macos.rinzu#L1041).

```rinzu
coalesced: Int
```

### field ticks

[source](../../sdk/macos/src/macos.rinzu#L1042).

```rinzu
ticks: Int
```

### field retries

[source](../../sdk/macos/src/macos.rinzu#L1043).

```rinzu
retries: Int
```

### field pending

[source](../../sdk/macos/src/macos.rinzu#L1044).

```rinzu
pending: CBool
```

### field visible

[source](../../sdk/macos/src/macos.rinzu#L1045).

```rinzu
visible: CBool
```

### field animating

[source](../../sdk/macos/src/macos.rinzu#L1046).

```rinzu
animating: CBool
```

### field stopped

[source](../../sdk/macos/src/macos.rinzu#L1047).

```rinzu
stopped: CBool
```

## rinzu.macos.Label

class · [source](../../sdk/macos/src/macos.rinzu#L2554).

```rinzu
class Label
```

### field view

[source](../../sdk/macos/src/macos.rinzu#L2555).

```rinzu
const view: View
```

## rinzu.macos.Label.destroy

Checked type: `fn(Label)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2561).

```rinzu
fn destroy(self: Label) -> Int
```

## rinzu.macos.Label.identity

Checked type: `fn(Label)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2557).

```rinzu
fn identity(self: Label) -> Int
```

## rinzu.macos.Label.setEnabled

Checked type: `fn(Label,Bool)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2565).

```rinzu
fn setEnabled(self: Label, enabled: Bool) -> Int
```

## rinzu.macos.Label.setText

Checked type: `fn(Label,String)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2573).

```rinzu
fn setText(self: Label, value: String) -> Int
```

## rinzu.macos.Label.text

Checked type: `fn(Label)->String`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2569).

```rinzu
fn text(self: Label) -> String
```

## rinzu.macos.LayoutItem

class · [source](../../sdk/macos/src/macos.rinzu#L3746).

```rinzu
class LayoutItem
```

Fixed extent plus a nonnegative proportional share of remaining space.

### field view

[source](../../sdk/macos/src/macos.rinzu#L3747).

```rinzu
const view: View
```

### field fixed

[source](../../sdk/macos/src/macos.rinzu#L3748).

```rinzu
const fixed: Float
```

### field grow

[source](../../sdk/macos/src/macos.rinzu#L3749).

```rinzu
const grow: Float
```

## rinzu.macos.Library

class · [source](../../sdk/macos/src/macos.rinzu#L425).

```rinzu
class Library
```

### field handle

[source](../../sdk/macos/src/macos.rinzu#L426).

```rinzu
const handle: LibraryHandle
```

## rinzu.macos.Library.computePipeline

Checked type: `fn(Library,String)->ComputePipeline`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L428).

```rinzu
fn computePipeline(self: Library, name: String) -> ComputePipeline
```

## rinzu.macos.Library.destroy

Checked type: `fn(Library)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L456).

```rinzu
fn destroy(self: Library) -> Int
```

## rinzu.macos.Library.renderPipeline

Checked type: `fn(Library,String,String)->RenderPipeline`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L437).

```rinzu
fn renderPipeline(self: Library, vertex: String, fragment: String) -> RenderPipeline
```

## rinzu.macos.Library.renderPipelineWithOptions

Checked type: `fn(Library,String,String,Int,Int)->RenderPipeline`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L460).

```rinzu
fn renderPipelineWithOptions(self: Library, vertex: String, fragment: String, format: Int, blend: Int)
        -> RenderPipeline
```

## rinzu.macos.LibraryHandle

Checked type: `LibraryHandle`.

opaque · [source](../../sdk/macos/src/macos.rinzu#L996).

```rinzu
opaque LibraryHandle;
```

## rinzu.macos.Menu

class · [source](../../sdk/macos/src/macos.rinzu#L475).

```rinzu
class Menu
```

### field handle

[source](../../sdk/macos/src/macos.rinzu#L476).

```rinzu
const handle: MenuHandle
```

## rinzu.macos.Menu.addAction

Checked type: `fn(Menu,String,Int,String,Int)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L478).

```rinzu
fn addAction(self: Menu, title: String, action: Int, key: String, modifiers: Int) -> Int
```

## rinzu.macos.Menu.addSeparator

Checked type: `fn(Menu)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L522).

```rinzu
fn addSeparator(self: Menu) -> Int
```

## rinzu.macos.Menu.addStandard

Checked type: `fn(Menu,String,Int,String,Int)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L498).

```rinzu
fn addStandard(self: Menu, title: String, command: Int, key: String, modifiers: Int) -> Int
```

## rinzu.macos.Menu.addSubmenu

Checked type: `fn(Menu,Menu)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L518).

```rinzu
fn addSubmenu(self: Menu, child: Menu) -> Int
```

## rinzu.macos.Menu.destroy

Checked type: `fn(Menu)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L530).

```rinzu
fn destroy(self: Menu) -> Int
```

## rinzu.macos.Menu.setState

Checked type: `fn(Menu,Int,Bool,Bool)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L526).

```rinzu
fn setState(self: Menu, action: Int, enabled: Bool, isChecked: Bool) -> Int
```

## rinzu.macos.MenuHandle

Checked type: `MenuHandle`.

opaque · [source](../../sdk/macos/src/macos.rinzu#L1004).

```rinzu
opaque MenuHandle;
```

## rinzu.macos.Quad2D

class · [source](../../sdk/macos/src/macos.rinzu#L3111).

```rinzu
class Quad2D
```

Six vertices per quad; quad index (not a byte or vertex offset).

### field destination

[source](../../sdk/macos/src/macos.rinzu#L3112).

```rinzu
const destination: Rect
```

### field uv

[source](../../sdk/macos/src/macos.rinzu#L3113).

```rinzu
const uv: Rect
```

### field tint

[source](../../sdk/macos/src/macos.rinzu#L3114).

```rinzu
const tint: Color
```

### field transform

[source](../../sdk/macos/src/macos.rinzu#L3115).

```rinzu
const transform: Transform2D
```

## rinzu.macos.Rect

Checked type: `Rect`.

native-record · [source](../../sdk/macos/src/macos.rinzu#L1008).

```rinzu
record Rect
```

### field x

[source](../../sdk/macos/src/macos.rinzu#L1009).

```rinzu
x: Float
```

### field y

[source](../../sdk/macos/src/macos.rinzu#L1010).

```rinzu
y: Float
```

### field width

[source](../../sdk/macos/src/macos.rinzu#L1011).

```rinzu
width: Float
```

### field height

[source](../../sdk/macos/src/macos.rinzu#L1012).

```rinzu
height: Float
```

## rinzu.macos.RenderPipeline

class · [source](../../sdk/macos/src/macos.rinzu#L535).

```rinzu
class RenderPipeline
```

### field handle

[source](../../sdk/macos/src/macos.rinzu#L536).

```rinzu
const handle: RenderPipelineHandle
```

## rinzu.macos.RenderPipeline.destroy

Checked type: `fn(RenderPipeline)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L538).

```rinzu
fn destroy(self: RenderPipeline) -> Int
```

## rinzu.macos.RenderPipelineHandle

Checked type: `RenderPipelineHandle`.

opaque · [source](../../sdk/macos/src/macos.rinzu#L1000).

```rinzu
opaque RenderPipelineHandle;
```

## rinzu.macos.RunContext

Checked type: `{session:Session}`.

type-alias · [source](../../sdk/macos/src/macos.rinzu#L973).

```rinzu
type RunContext = { session: Session };
```

## rinzu.macos.Sampler

class · [source](../../sdk/macos/src/macos.rinzu#L543).

```rinzu
class Sampler
```

### field handle

[source](../../sdk/macos/src/macos.rinzu#L544).

```rinzu
const handle: SamplerHandle
```

## rinzu.macos.Sampler.destroy

Checked type: `fn(Sampler)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L546).

```rinzu
fn destroy(self: Sampler) -> Int
```

## rinzu.macos.SamplerHandle

Checked type: `SamplerHandle`.

opaque · [source](../../sdk/macos/src/macos.rinzu#L988).

```rinzu
opaque SamplerHandle;
```

## rinzu.macos.Scissor

Checked type: `Scissor`.

native-record · [source](../../sdk/macos/src/macos.rinzu#L1105).

```rinzu
record Scissor
```

### field x

[source](../../sdk/macos/src/macos.rinzu#L1106).

```rinzu
x: Int
```

### field y

[source](../../sdk/macos/src/macos.rinzu#L1107).

```rinzu
y: Int
```

### field width

[source](../../sdk/macos/src/macos.rinzu#L1108).

```rinzu
width: Int
```

### field height

[source](../../sdk/macos/src/macos.rinzu#L1109).

```rinzu
height: Int
```

## rinzu.macos.ScrollView

class · [source](../../sdk/macos/src/macos.rinzu#L2721).

```rinzu
class ScrollView
```

### field view

[source](../../sdk/macos/src/macos.rinzu#L2722).

```rinzu
const view: View
```

## rinzu.macos.ScrollView.destroy

Checked type: `fn(ScrollView)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2728).

```rinzu
fn destroy(self: ScrollView) -> Int
```

## rinzu.macos.ScrollView.identity

Checked type: `fn(ScrollView)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2724).

```rinzu
fn identity(self: ScrollView) -> Int
```

## rinzu.macos.ScrollView.scrollTo

Checked type: `fn(ScrollView,Float,Float)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2740).

```rinzu
fn scrollTo(self: ScrollView, x: Float, y: Float) -> Int
```

## rinzu.macos.ScrollView.setContent

Checked type: `fn(ScrollView,View)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2736).

```rinzu
fn setContent(self: ScrollView, content: View) -> Int
```

## rinzu.macos.ScrollView.setEnabled

Checked type: `fn(ScrollView,Bool)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2732).

```rinzu
fn setEnabled(self: ScrollView, enabled: Bool) -> Int
```

## rinzu.macos.ScrollView.visibleRect

Checked type: `fn(ScrollView)->Rect`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2744).

```rinzu
fn visibleRect(self: ScrollView) -> Rect
```

## rinzu.macos.Selection

Checked type: `Selection`.

native-record · [source](../../sdk/macos/src/macos.rinzu#L1163).

```rinzu
record Selection
```

### field start

[source](../../sdk/macos/src/macos.rinzu#L1164).

```rinzu
start: Int
```

### field end

[source](../../sdk/macos/src/macos.rinzu#L1165).

```rinzu
end: Int
```

## rinzu.macos.Session

class · [source](../../sdk/macos/src/macos.rinzu#L551).

```rinzu
class Session
```

### field handle

[source](../../sdk/macos/src/macos.rinzu#L552).

```rinzu
const handle: SessionHandle
```

## rinzu.macos.Session.button

Checked type: `fn(Session,String,Rect,Int)->Button`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L622).

```rinzu
fn button(self: Session, title: String, frame: Rect, action: Int) -> Button
```

## rinzu.macos.Session.checkbox

Checked type: `fn(Session,String,Rect,Int)->Checkbox`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L626).

```rinzu
fn checkbox(self: Session, title: String, frame: Rect, action: Int) -> Checkbox
```

## rinzu.macos.Session.clipboardText

Checked type: `fn(Session)->String`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L647).

```rinzu
fn clipboardText(self: Session) -> String
```

## rinzu.macos.Session.close

Checked type: `async(Session)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L614).

```rinzu
async fn close(self: Session) -> Int
```

## rinzu.macos.Session.device

Checked type: `fn(Session)->Device`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L602).

```rinzu
fn device(self: Session) -> Device
```

## rinzu.macos.Session.eventSequence

Checked type: `fn(Session)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L661).

```rinzu
fn eventSequence(self: Session) -> Int
```

Sequence barrier for reconciling a native snapshot with already queued input.

## rinzu.macos.Session.font

Checked type: `fn(Session,String,Float,Bool)->Font`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L665).

```rinzu
fn font(self: Session, name: String, size: Float, bold: Bool) -> Font
```

## rinzu.macos.Session.isRunning

Checked type: `fn(Session)->Bool`.

Error contract: `nothrow` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L577).

```rinzu
fn isRunning(self: Session) -> Bool
```

## rinzu.macos.Session.label

Checked type: `fn(Session,String,Rect)->Label`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L618).

```rinzu
fn label(self: Session, title: String, frame: Rect) -> Label
```

## rinzu.macos.Session.nextEvent

Checked type: `async(Session)->UiEvent`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L585).

```rinzu
async fn nextEvent(self: Session) -> UiEvent
```

## rinzu.macos.Session.resourcePath

Checked type: `fn(Session,String)->String`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L564).

```rinzu
fn resourcePath(self: Session, relative: String) -> String
```

Resolve a normalized app-relative resource name without changing cwd.

## rinzu.macos.Session.resourceRoot

Checked type: `fn(Session)->Option<String>`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L555).

```rinzu
fn resourceRoot(self: Session) -> Option<String>
```

Owned absolute app-resource root; None in an ordinary JIT/console host.

## rinzu.macos.Session.run

Checked type: `async(Session,fn($rinzu_record_f7c713f12e29ebb74f67222ddd835128,Event)->Unit)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L606).

```rinzu
async fn run(self: Session, handler: fn (RunContext, Event) -> Unit) -> Int
```

## rinzu.macos.Session.runEvents

Checked type: `fn(Session)->Future<Int>`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L581).

```rinzu
fn runEvents(self: Session) -> Future<Int>
```

## rinzu.macos.Session.scrollView

Checked type: `fn(Session,Rect)->ScrollView`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L638).

```rinzu
fn scrollView(self: Session, frame: Rect) -> ScrollView
```

## rinzu.macos.Session.setClipboardText

Checked type: `fn(Session,String)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L642).

```rinzu
fn setClipboardText(self: Session, value: String) -> Int
```

## rinzu.macos.Session.stop

Checked type: `fn(Session)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L610).

```rinzu
fn stop(self: Session) -> Int
```

## rinzu.macos.Session.textField

Checked type: `fn(Session,String,Rect,Int)->TextField`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L630).

```rinzu
fn textField(self: Session, title: String, frame: Rect, action: Int) -> TextField
```

## rinzu.macos.Session.textView

Checked type: `fn(Session,String,Rect)->TextView`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L634).

```rinzu
fn textView(self: Session, title: String, frame: Rect) -> TextView
```

## rinzu.macos.Session.view

Checked type: `fn(Session,Rect)->View`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L598).

```rinzu
fn view(self: Session, rect: Rect) -> View
```

## rinzu.macos.Session.window

Checked type: `fn(Session,String,Float,Float)->Window`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L589).

```rinzu
fn window(self: Session, title: String, width: Float, height: Float) -> Window
```

## rinzu.macos.SessionHandle

Checked type: `SessionHandle`.

opaque · [source](../../sdk/macos/src/macos.rinzu#L976).

```rinzu
opaque SessionHandle;
```

## rinzu.macos.Statistics

Checked type: `Statistics`.

native-record · [source](../../sdk/macos/src/macos.rinzu#L1133).

```rinzu
record Statistics
```

### field sessions

[source](../../sdk/macos/src/macos.rinzu#L1134).

```rinzu
sessions: Int
```

### field windows

[source](../../sdk/macos/src/macos.rinzu#L1135).

```rinzu
windows: Int
```

### field views

[source](../../sdk/macos/src/macos.rinzu#L1136).

```rinzu
views: Int
```

### field devices

[source](../../sdk/macos/src/macos.rinzu#L1137).

```rinzu
devices: Int
```

### field buffers

[source](../../sdk/macos/src/macos.rinzu#L1138).

```rinzu
buffers: Int
```

### field libraries

[source](../../sdk/macos/src/macos.rinzu#L1139).

```rinzu
libraries: Int
```

### field pipelines

[source](../../sdk/macos/src/macos.rinzu#L1140).

```rinzu
pipelines: Int
```

### field commands

[source](../../sdk/macos/src/macos.rinzu#L1141).

```rinzu
commands: Int
```

### field bufferBytes

[source](../../sdk/macos/src/macos.rinzu#L1142).

```rinzu
bufferBytes: Int
```

### field retainedControls

[source](../../sdk/macos/src/macos.rinzu#L1143).

```rinzu
retainedControls: Int
```

### field queuedEvents

[source](../../sdk/macos/src/macos.rinzu#L1144).

```rinzu
queuedEvents: Int
```

### field eventPayloadBytes

[source](../../sdk/macos/src/macos.rinzu#L1145).

```rinzu
eventPayloadBytes: Int
```

### field snapshotBytes

[source](../../sdk/macos/src/macos.rinzu#L1146).

```rinzu
snapshotBytes: Int
```

### field sheets

[source](../../sdk/macos/src/macos.rinzu#L1147).

```rinzu
sheets: Int
```

### field sheetBytes

[source](../../sdk/macos/src/macos.rinzu#L1148).

```rinzu
sheetBytes: Int
```

### field textures

[source](../../sdk/macos/src/macos.rinzu#L1149).

```rinzu
textures: Int
```

### field samplers

[source](../../sdk/macos/src/macos.rinzu#L1150).

```rinzu
samplers: Int
```

### field textureBytes

[source](../../sdk/macos/src/macos.rinzu#L1151).

```rinzu
textureBytes: Int
```

### field fonts

[source](../../sdk/macos/src/macos.rinzu#L1152).

```rinzu
fonts: Int
```

### field textLayouts

[source](../../sdk/macos/src/macos.rinzu#L1153).

```rinzu
textLayouts: Int
```

### field textBytes

[source](../../sdk/macos/src/macos.rinzu#L1154).

```rinzu
textBytes: Int
```

### field frameClocks

[source](../../sdk/macos/src/macos.rinzu#L1155).

```rinzu
frameClocks: Int
```

### field frameTimers

[source](../../sdk/macos/src/macos.rinzu#L1156).

```rinzu
frameTimers: Int
```

### field frameWaiters

[source](../../sdk/macos/src/macos.rinzu#L1157).

```rinzu
frameWaiters: Int
```

### field dataRows

[source](../../sdk/macos/src/macos.rinzu#L1158).

```rinzu
dataRows: Int
```

### field dataBytes

[source](../../sdk/macos/src/macos.rinzu#L1159).

```rinzu
dataBytes: Int
```

### field dataUpdates

[source](../../sdk/macos/src/macos.rinzu#L1160).

```rinzu
dataUpdates: Int
```

## rinzu.macos.Text2D

class · [source](../../sdk/macos/src/macos.rinzu#L3502).

```rinzu
class Text2D
```

Reusable text snapshot. Keep it across frames; reraster only after text/style changes.

### field layout

[source](../../sdk/macos/src/macos.rinzu#L3503).

```rinzu
const layout: TextLayout
```

### field texture

[source](../../sdk/macos/src/macos.rinzu#L3504).

```rinzu
const texture: Texture
```

### field metrics

[source](../../sdk/macos/src/macos.rinzu#L3505).

```rinzu
const metrics: TextMetrics
```

### field scale

[source](../../sdk/macos/src/macos.rinzu#L3506).

```rinzu
const scale: Float
```

### field width

[source](../../sdk/macos/src/macos.rinzu#L3507).

```rinzu
const width: Int
```

### field height

[source](../../sdk/macos/src/macos.rinzu#L3508).

```rinzu
const height: Int
```

## rinzu.macos.Text2D.destroy

Checked type: `fn(Text2D)->Unit`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L3510).

```rinzu
fn destroy(self: Text2D)
```

## rinzu.macos.TextField

class · [source](../../sdk/macos/src/macos.rinzu#L2645).

```rinzu
class TextField
```

### field view

[source](../../sdk/macos/src/macos.rinzu#L2646).

```rinzu
const view: View
```

## rinzu.macos.TextField.destroy

Checked type: `fn(TextField)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2652).

```rinzu
fn destroy(self: TextField) -> Int
```

## rinzu.macos.TextField.identity

Checked type: `fn(TextField)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2648).

```rinzu
fn identity(self: TextField) -> Int
```

## rinzu.macos.TextField.isComposing

Checked type: `fn(TextField)->Bool`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2676).

```rinzu
fn isComposing(self: TextField) -> Bool
```

## rinzu.macos.TextField.select

Checked type: `fn(TextField,Int,Int)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2672).

```rinzu
fn select(self: TextField, start: Int, end: Int) -> Int
```

## rinzu.macos.TextField.selection

Checked type: `fn(TextField)->Selection`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2668).

```rinzu
fn selection(self: TextField) -> Selection
```

## rinzu.macos.TextField.setEnabled

Checked type: `fn(TextField,Bool)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2656).

```rinzu
fn setEnabled(self: TextField, enabled: Bool) -> Int
```

## rinzu.macos.TextField.setText

Checked type: `fn(TextField,String)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2664).

```rinzu
fn setText(self: TextField, value: String) -> Int
```

## rinzu.macos.TextField.text

Checked type: `fn(TextField)->String`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2660).

```rinzu
fn text(self: TextField) -> String
```

## rinzu.macos.TextLayout

class · [source](../../sdk/macos/src/macos.rinzu#L675).

```rinzu
class TextLayout
```

### field handle

[source](../../sdk/macos/src/macos.rinzu#L676).

```rinzu
const handle: TextLayoutHandle
```

## rinzu.macos.TextLayout.destroy

Checked type: `fn(TextLayout)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L696).

```rinzu
fn destroy(self: TextLayout) -> Int
```

## rinzu.macos.TextLayout.metrics

Checked type: `fn(TextLayout)->TextMetrics`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L678).

```rinzu
fn metrics(self: TextLayout) -> TextMetrics
```

## rinzu.macos.TextLayoutHandle

Checked type: `TextLayoutHandle`.

opaque · [source](../../sdk/macos/src/macos.rinzu#L992).

```rinzu
opaque TextLayoutHandle;
```

## rinzu.macos.TextMetrics

Checked type: `TextMetrics`.

native-record · [source](../../sdk/macos/src/macos.rinzu#L1050).

```rinzu
record TextMetrics
```

### field width

[source](../../sdk/macos/src/macos.rinzu#L1051).

```rinzu
width: Float
```

### field height

[source](../../sdk/macos/src/macos.rinzu#L1052).

```rinzu
height: Float
```

### field baseline

[source](../../sdk/macos/src/macos.rinzu#L1053).

```rinzu
baseline: Float
```

### field ascent

[source](../../sdk/macos/src/macos.rinzu#L1054).

```rinzu
ascent: Float
```

### field descent

[source](../../sdk/macos/src/macos.rinzu#L1055).

```rinzu
descent: Float
```

### field leading

[source](../../sdk/macos/src/macos.rinzu#L1056).

```rinzu
leading: Float
```

### field left

[source](../../sdk/macos/src/macos.rinzu#L1057).

```rinzu
left: Float
```

### field top

[source](../../sdk/macos/src/macos.rinzu#L1058).

```rinzu
top: Float
```

### field rasterWidth

[source](../../sdk/macos/src/macos.rinzu#L1059).

```rinzu
rasterWidth: Float
```

### field rasterHeight

[source](../../sdk/macos/src/macos.rinzu#L1060).

```rinzu
rasterHeight: Float
```

### field lines

[source](../../sdk/macos/src/macos.rinzu#L1061).

```rinzu
lines: Int
```

## rinzu.macos.TextView

class · [source](../../sdk/macos/src/macos.rinzu#L2683).

```rinzu
class TextView
```

### field view

[source](../../sdk/macos/src/macos.rinzu#L2684).

```rinzu
const view: View
```

## rinzu.macos.TextView.destroy

Checked type: `fn(TextView)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2690).

```rinzu
fn destroy(self: TextView) -> Int
```

## rinzu.macos.TextView.identity

Checked type: `fn(TextView)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2686).

```rinzu
fn identity(self: TextView) -> Int
```

## rinzu.macos.TextView.isComposing

Checked type: `fn(TextView)->Bool`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2714).

```rinzu
fn isComposing(self: TextView) -> Bool
```

## rinzu.macos.TextView.select

Checked type: `fn(TextView,Int,Int)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2710).

```rinzu
fn select(self: TextView, start: Int, end: Int) -> Int
```

## rinzu.macos.TextView.selection

Checked type: `fn(TextView)->Selection`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2706).

```rinzu
fn selection(self: TextView) -> Selection
```

## rinzu.macos.TextView.setEnabled

Checked type: `fn(TextView,Bool)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2694).

```rinzu
fn setEnabled(self: TextView, enabled: Bool) -> Int
```

## rinzu.macos.TextView.setText

Checked type: `fn(TextView,String)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2702).

```rinzu
fn setText(self: TextView, value: String) -> Int
```

## rinzu.macos.TextView.text

Checked type: `fn(TextView)->String`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2698).

```rinzu
fn text(self: TextView) -> String
```

## rinzu.macos.Texture

class · [source](../../sdk/macos/src/macos.rinzu#L701).

```rinzu
class Texture
```

### field handle

[source](../../sdk/macos/src/macos.rinzu#L702).

```rinzu
const handle: TextureHandle
```

## rinzu.macos.Texture.destroy

Checked type: `fn(Texture)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L723).

```rinzu
fn destroy(self: Texture) -> Int
```

## rinzu.macos.Texture.identity

Checked type: `fn(Texture)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L727).

```rinzu
fn identity(self: Texture) -> Int
```

## rinzu.macos.Texture.info

Checked type: `fn(Texture)->TextureInfo`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L704).

```rinzu
fn info(self: Texture) -> TextureInfo
```

## rinzu.macos.Texture.read

Checked type: `fn(Texture,Scissor)->ByteArray`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L718).

```rinzu
fn read(self: Texture, region: Scissor) -> ByteArray
```

Tightly packed owned bytes, in the texture's channel order; requires no GPU lease.

## rinzu.macos.Texture.write

Checked type: `fn(Texture,Scissor,Int,ByteArray)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L710).

```rinzu
fn write(self: Texture, region: Scissor, rowBytes: Int, bytes: ByteArray) -> Int
```

## rinzu.macos.TextureHandle

Checked type: `TextureHandle`.

opaque · [source](../../sdk/macos/src/macos.rinzu#L986).

```rinzu
opaque TextureHandle;
```

## rinzu.macos.TextureInfo

Checked type: `TextureInfo`.

native-record · [source](../../sdk/macos/src/macos.rinzu#L1022).

```rinzu
record TextureInfo
```

### field width

[source](../../sdk/macos/src/macos.rinzu#L1023).

```rinzu
width: Int
```

### field height

[source](../../sdk/macos/src/macos.rinzu#L1024).

```rinzu
height: Int
```

### field format

[source](../../sdk/macos/src/macos.rinzu#L1025).

```rinzu
format: Int
```

### field usage

[source](../../sdk/macos/src/macos.rinzu#L1026).

```rinzu
usage: Int
```

### field bytes

[source](../../sdk/macos/src/macos.rinzu#L1027).

```rinzu
bytes: Int
```

## rinzu.macos.Transform2D

Checked type: `Transform2D`.

native-record · [source](../../sdk/macos/src/macos.rinzu#L1075).

```rinzu
record Transform2D
```

### field a

[source](../../sdk/macos/src/macos.rinzu#L1076).

```rinzu
a: Float
```

### field b

[source](../../sdk/macos/src/macos.rinzu#L1077).

```rinzu
b: Float
```

### field c

[source](../../sdk/macos/src/macos.rinzu#L1078).

```rinzu
c: Float
```

### field d

[source](../../sdk/macos/src/macos.rinzu#L1079).

```rinzu
d: Float
```

### field tx

[source](../../sdk/macos/src/macos.rinzu#L1080).

```rinzu
tx: Float
```

### field ty

[source](../../sdk/macos/src/macos.rinzu#L1081).

```rinzu
ty: Float
```

## rinzu.macos.UiAction

class · [source](../../sdk/macos/src/macos.rinzu#L3667).

```rinzu
class UiAction
```

Source-only handler storage. A binding admits one active job at a time.

### field action

[source](../../sdk/macos/src/macos.rinzu#L3668).

```rinzu
const action: Int
```

### field handler

[source](../../sdk/macos/src/macos.rinzu#L3669).

```rinzu
const handler: fn (UiJob, UiEvent) -> Future<Unit>
```

### field busy

[source](../../sdk/macos/src/macos.rinzu#L3670).

```rinzu
busy: Bool
```

## rinzu.macos.UiActions

class · [source](../../sdk/macos/src/macos.rinzu#L3673).

```rinzu
class UiActions
```

### field bindings

[source](../../sdk/macos/src/macos.rinzu#L3674).

```rinzu
bindings: List<UiAction>
```

## rinzu.macos.UiActions.handle

Checked type: `fn(UiActions,UiScope,UiEvent)->Unit`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L3692).

```rinzu
fn handle(self: UiActions, scope: UiScope, event: UiEvent)
```

## rinzu.macos.UiActions.on

Checked type: `fn(UiActions,Int,$rinzu_closure_735ce3cd6d5e35d7)->Unit`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L3676).

```rinzu
fn on(self: UiActions, action: Int, handler: fn (UiJob, UiEvent) -> Future<Unit>)
```

## rinzu.macos.UiApplication

class · [source](../../sdk/macos/src/macos.rinzu#L2402).

```rinzu
class UiApplication
```

Application body context. Each Window keeps its own UiScope.

### field session

[source](../../sdk/macos/src/macos.rinzu#L2403).

```rinzu
const session: Session
```

### field primary

[source](../../sdk/macos/src/macos.rinzu#L2404).

```rinzu
const primary: UiScope
```

## rinzu.macos.UiControl

class · [source](../../sdk/macos/src/macos.rinzu#L1939).

```rinzu
class UiControl
```

Shared stop state contains no task handles.

### field stopping

[source](../../sdk/macos/src/macos.rinzu#L1940).

```rinzu
stopping: Bool
```

### field closed

[source](../../sdk/macos/src/macos.rinzu#L1941).

```rinzu
closed: Bool
```

## rinzu.macos.UiEvent

class · [source](../../sdk/macos/src/macos.rinzu#L1668).

```rinzu
class UiEvent
```

An owned event. Text remains valid after the native queue is acknowledged.

### field kind

[source](../../sdk/macos/src/macos.rinzu#L1669).

```rinzu
const kind: Int
```

### field windowId

[source](../../sdk/macos/src/macos.rinzu#L1670).

```rinzu
const windowId: Int
```

### field viewId

[source](../../sdk/macos/src/macos.rinzu#L1671).

```rinzu
const viewId: Int
```

### field sequence

[source](../../sdk/macos/src/macos.rinzu#L1672).

```rinzu
const sequence: Int
```

### field x

[source](../../sdk/macos/src/macos.rinzu#L1673).

```rinzu
const x: Float
```

### field y

[source](../../sdk/macos/src/macos.rinzu#L1674).

```rinzu
const y: Float
```

### field width

[source](../../sdk/macos/src/macos.rinzu#L1675).

```rinzu
const width: Float
```

### field height

[source](../../sdk/macos/src/macos.rinzu#L1676).

```rinzu
const height: Float
```

### field scale

[source](../../sdk/macos/src/macos.rinzu#L1677).

```rinzu
const scale: Float
```

### field keyCode

[source](../../sdk/macos/src/macos.rinzu#L1678).

```rinzu
const keyCode: Int
```

### field modifiers

[source](../../sdk/macos/src/macos.rinzu#L1679).

```rinzu
const modifiers: Int
```

### field action

[source](../../sdk/macos/src/macos.rinzu#L1680).

```rinzu
const action: Int
```

### field selectionStart

[source](../../sdk/macos/src/macos.rinzu#L1681).

```rinzu
const selectionStart: Int
```

### field selectionEnd

[source](../../sdk/macos/src/macos.rinzu#L1682).

```rinzu
const selectionEnd: Int
```

### field flags

[source](../../sdk/macos/src/macos.rinzu#L1683).

```rinzu
const flags: Int
```

### field value

[source](../../sdk/macos/src/macos.rinzu#L1684).

```rinzu
const value: Float
```

### field deltaX

[source](../../sdk/macos/src/macos.rinzu#L1685).

```rinzu
const deltaX: Float
```

### field deltaY

[source](../../sdk/macos/src/macos.rinzu#L1686).

```rinzu
const deltaY: Float
```

### field text

[source](../../sdk/macos/src/macos.rinzu#L1687).

```rinzu
const text: String
```

## rinzu.macos.UiJob

class · [source](../../sdk/macos/src/macos.rinzu#L1945).

```rinzu
class UiJob
```

A job receives native UI capabilities and stop state, without the job registry.

### field session

[source](../../sdk/macos/src/macos.rinzu#L1946).

```rinzu
const session: Session
```

### field window

[source](../../sdk/macos/src/macos.rinzu#L1947).

```rinzu
const window: Window
```

### field windowId

[source](../../sdk/macos/src/macos.rinzu#L1948).

```rinzu
const windowId: Int
```

### field control

[source](../../sdk/macos/src/macos.rinzu#L1949).

```rinzu
const control: UiControl
```

## rinzu.macos.UiJob.isStopping

Checked type: `fn(UiJob)->Bool`.

Error contract: `nothrow` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L1951).

```rinzu
fn isStopping(self: UiJob) -> Bool
```

## rinzu.macos.UiJob.requestStop

Checked type: `fn(UiJob)->Unit`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L1955).

```rinzu
fn requestStop(self: UiJob)
```

## rinzu.macos.UiJob.wake

Checked type: `fn(UiJob)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L1960).

```rinzu
fn wake(self: UiJob) -> Int
```

## rinzu.macos.UiScope

class · [source](../../sdk/macos/src/macos.rinzu#L1969).

```rinzu
class UiScope
```

Main-owned Window resources and Source jobs. Jobs receive a UiJob context.

### field session

[source](../../sdk/macos/src/macos.rinzu#L1970).

```rinzu
const session: Session
```

### field window

[source](../../sdk/macos/src/macos.rinzu#L1971).

```rinzu
const window: Window
```

### field windowId

[source](../../sdk/macos/src/macos.rinzu#L1972).

```rinzu
const windowId: Int
```

### field tasks

[source](../../sdk/macos/src/macos.rinzu#L1973).

```rinzu
tasks: List<UiTask>
```

### field canvases

[source](../../sdk/macos/src/macos.rinzu#L1974).

```rinzu
canvases: List<Canvas2D>
```

### field errors

[source](../../sdk/macos/src/macos.rinzu#L1975).

```rinzu
errors: List<Error>
```

### field reportedErrors

[source](../../sdk/macos/src/macos.rinzu#L1976).

```rinzu
reportedErrors: Int
```

### field control

[source](../../sdk/macos/src/macos.rinzu#L1977).

```rinzu
const control: UiControl
```

## rinzu.macos.UiScope.button

Checked type: `fn(UiScope,String,Rect,Int)->Button`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2177).

```rinzu
fn button(self: UiScope, title: String, frame: Rect, action: Int) -> Button
```

## rinzu.macos.UiScope.canvas2D

Checked type: `fn(UiScope,Device,Rect,Int,Int,Color,$rinzu_closure_efe2dcc12b934013)->Canvas2D`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2076).

```rinzu
fn canvas2D(
        self: UiScope,
        device: Device,
        frame: Rect,
        capacity: Int,
        fps: Int,
        clear: Color,
        draw: fn (Frame2D) -> Future<Unit>
    ) -> Canvas2D
```

A scope-owned drawing job. Draw callbacks stay in Source, never in AppKit.

## rinzu.macos.UiScope.checkbox

Checked type: `fn(UiScope,String,Rect,Int)->Checkbox`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2181).

```rinzu
fn checkbox(self: UiScope, title: String, frame: Rect, action: Int) -> Checkbox
```

## rinzu.macos.UiScope.close

Checked type: `async(UiScope)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2036).

```rinzu
async fn close(self: UiScope) -> Int
```

## rinzu.macos.UiScope.dispatch

Checked type: `async(UiScope,$rinzu_closure_6d2f0d3cfa7112a6)->Unit`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2024).

```rinzu
async fn dispatch(self: UiScope, handler: fn (UiScope, UiEvent) -> Future<Unit>)
```

The Application has one reader. Close/quit policy stays in the Source handler.

## rinzu.macos.UiScope.dispatchActions

Checked type: `async(UiScope,UiActions)->Unit`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2165).

```rinzu
async fn dispatchActions(self: UiScope, actions: UiActions)
```

## rinzu.macos.UiScope.isStopping

Checked type: `fn(UiScope)->Bool`.

Error contract: `nothrow` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2000).

```rinzu
fn isStopping(self: UiScope) -> Bool
```

## rinzu.macos.UiScope.join

Checked type: `async(UiScope)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2018).

```rinzu
async fn join(self: UiScope) -> Int
```

Join the current jobs without closing the Window; callers stop job admission first.

## rinzu.macos.UiScope.label

Checked type: `fn(UiScope,String,Rect)->Label`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2173).

```rinzu
fn label(self: UiScope, title: String, frame: Rect) -> Label
```

## rinzu.macos.UiScope.listView

Checked type: `fn(UiScope,Rect,Int)->DataView`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2063).

```rinzu
fn listView(self: UiScope, frame: Rect, action: Int) -> DataView
```

## rinzu.macos.UiScope.menu

Checked type: `fn(UiScope,String)->Menu`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2197).

```rinzu
fn menu(self: UiScope, title: String) -> Menu
```

## rinzu.macos.UiScope.requestStop

Checked type: `fn(UiScope)->Unit`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2004).

```rinzu
fn requestStop(self: UiScope)
```

## rinzu.macos.UiScope.scrollView

Checked type: `fn(UiScope,Rect)->ScrollView`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2193).

```rinzu
fn scrollView(self: UiScope, frame: Rect) -> ScrollView
```

## rinzu.macos.UiScope.start

Checked type: `fn(UiScope,$rinzu_closure_2671e076e7dc3bc9)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L1979).

```rinzu
fn start(self: UiScope, work: fn (UiJob) -> Future<Unit>) -> Int
```

## rinzu.macos.UiScope.surface

Checked type: `fn(UiScope,Device,Rect)->View`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2057).

```rinzu
fn surface(self: UiScope, device: Device, frame: Rect) -> View
```

## rinzu.macos.UiScope.tableView

Checked type: `fn(UiScope,List<DataColumn>,Rect,Int)->DataView`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2067).

```rinzu
fn tableView(self: UiScope, columns: List<DataColumn>, frame: Rect, action: Int) -> DataView
```

## rinzu.macos.UiScope.textField

Checked type: `fn(UiScope,String,Rect,Int)->TextField`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2185).

```rinzu
fn textField(self: UiScope, title: String, frame: Rect, action: Int) -> TextField
```

## rinzu.macos.UiScope.textView

Checked type: `fn(UiScope,String,Rect)->TextView`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2189).

```rinzu
fn textView(self: UiScope, title: String, frame: Rect) -> TextView
```

## rinzu.macos.UiScope.treeView

Checked type: `fn(UiScope,List<DataColumn>,Rect,Int)->DataView`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2071).

```rinzu
fn treeView(self: UiScope, columns: List<DataColumn>, frame: Rect, action: Int) -> DataView
```

## rinzu.macos.UiScope.wake

Checked type: `fn(UiScope)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L2010).

```rinzu
fn wake(self: UiScope) -> Int
```

Wake the single dispatcher after Source state changes outside an input handler.

## rinzu.macos.UiTask

class · [source](../../sdk/macos/src/macos.rinzu#L1932).

```rinzu
class UiTask
```

A cooperative job registered with a UI scope. Prefer UiScope.start.

### field pending

[source](../../sdk/macos/src/macos.rinzu#L1933).

```rinzu
pending: Option<Future<Unit>>
```

### field done

[source](../../sdk/macos/src/macos.rinzu#L1934).

```rinzu
done: Bool
```

### field started

[source](../../sdk/macos/src/macos.rinzu#L1935).

```rinzu
started: Bool
```

## rinzu.macos.Vertex2D

Checked type: `Vertex2D`.

native-record · [source](../../sdk/macos/src/macos.rinzu#L1064).

```rinzu
record Vertex2D
```

### field x

[source](../../sdk/macos/src/macos.rinzu#L1065).

```rinzu
x: Float
```

### field y

[source](../../sdk/macos/src/macos.rinzu#L1066).

```rinzu
y: Float
```

### field u

[source](../../sdk/macos/src/macos.rinzu#L1067).

```rinzu
u: Float
```

### field v

[source](../../sdk/macos/src/macos.rinzu#L1068).

```rinzu
v: Float
```

### field red

[source](../../sdk/macos/src/macos.rinzu#L1069).

```rinzu
red: Float
```

### field green

[source](../../sdk/macos/src/macos.rinzu#L1070).

```rinzu
green: Float
```

### field blue

[source](../../sdk/macos/src/macos.rinzu#L1071).

```rinzu
blue: Float
```

### field alpha

[source](../../sdk/macos/src/macos.rinzu#L1072).

```rinzu
alpha: Float
```

## rinzu.macos.View

class · [source](../../sdk/macos/src/macos.rinzu#L732).

```rinzu
class View
```

### field handle

[source](../../sdk/macos/src/macos.rinzu#L733).

```rinzu
const handle: ViewHandle
```

## rinzu.macos.View.addSubview

Checked type: `fn(View,View)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L774).

```rinzu
fn addSubview(self: View, child: View) -> Int
```

## rinzu.macos.View.destroy

Checked type: `fn(View)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L782).

```rinzu
fn destroy(self: View) -> Int
```

## rinzu.macos.View.drawableSize

Checked type: `fn(View)->Rect`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L830).

```rinzu
fn drawableSize(self: View) -> Rect
```

## rinzu.macos.View.fill

Checked type: `fn(View,View,Float)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L818).

```rinzu
fn fill(self: View, parent: View, inset: Float) -> Int
```

Attach with constant insets and resize width/height with the parent.

## rinzu.macos.View.focus

Checked type: `fn(View)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L798).

```rinzu
fn focus(self: View) -> Int
```

## rinzu.macos.View.frame

Checked type: `fn(View)->Rect`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L764).

```rinzu
fn frame(self: View) -> Rect
```

## rinzu.macos.View.frameClock

Checked type: `fn(View,Int)->FrameClock`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L836).

```rinzu
fn frameClock(self: View, fps: Int) -> FrameClock
```

## rinzu.macos.View.identity

Checked type: `fn(View)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L760).

```rinzu
fn identity(self: View) -> Int
```

## rinzu.macos.View.isEnabled

Checked type: `fn(View)->Bool`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L802).

```rinzu
fn isEnabled(self: View) -> Bool
```

## rinzu.macos.View.isHidden

Checked type: `fn(View)->Bool`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L807).

```rinzu
fn isHidden(self: View) -> Bool
```

## rinzu.macos.View.removeFromSuperview

Checked type: `fn(View)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L778).

```rinzu
fn removeFromSuperview(self: View) -> Int
```

## rinzu.macos.View.requestDraw

Checked type: `fn(View)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L786).

```rinzu
fn requestDraw(self: View) -> Int
```

## rinzu.macos.View.setAccessibility

Checked type: `fn(View,String,String,String)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L735).

```rinzu
fn setAccessibility(self: View, label: String, help: String, identifier: String) -> Int
```

## rinzu.macos.View.setAutoresizing

Checked type: `fn(View,Int)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L813).

```rinzu
fn setAutoresizing(self: View, mask: Int) -> Int
```

AppKit bits: min-X margin 1, width 2, max-X 4, min-Y 8, height 16, max-Y 32.

## rinzu.macos.View.setEnabled

Checked type: `fn(View,Bool)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L790).

```rinzu
fn setEnabled(self: View, enabled: Bool) -> Int
```

## rinzu.macos.View.setFrame

Checked type: `fn(View,Rect)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L770).

```rinzu
fn setFrame(self: View, rect: Rect) -> Int
```

## rinzu.macos.View.setHidden

Checked type: `fn(View,Bool)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L794).

```rinzu
fn setHidden(self: View, hidden: Bool) -> Int
```

## rinzu.macos.View.setTooltip

Checked type: `fn(View,String)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L755).

```rinzu
fn setTooltip(self: View, text: String) -> Int
```

## rinzu.macos.ViewHandle

Checked type: `ViewHandle`.

opaque · [source](../../sdk/macos/src/macos.rinzu#L980).

```rinzu
opaque ViewHandle;
```

## rinzu.macos.Window

class · [source](../../sdk/macos/src/macos.rinzu#L841).

```rinzu
class Window
```

### field handle

[source](../../sdk/macos/src/macos.rinzu#L842).

```rinzu
const handle: WindowHandle
```

## rinzu.macos.Window.alert

Checked type: `async(Window,String,String,String,String,String)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L895).

```rinzu
async fn alert(self: Window, title: String, message: String, first: String, second: String, third: String) -> Int
```

One to three buttons, numbered 1..3. Empty optional titles omit buttons. Dismissal returns 0.

## rinzu.macos.Window.command

Checked type: `fn(Window,Int)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L880).

```rinzu
fn command(self: Window, command: Int) -> Int
```

## rinzu.macos.Window.contentView

Checked type: `fn(Window)->View`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L861).

```rinzu
fn contentView(self: Window) -> View
```

## rinzu.macos.Window.destroy

Checked type: `fn(Window)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L865).

```rinzu
fn destroy(self: Window) -> Int
```

## rinzu.macos.Window.dismissSheet

Checked type: `fn(Window)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L940).

```rinzu
fn dismissSheet(self: Window) -> Int
```

Dismiss the current sheet without closing the Window or stopping later sheets.

## rinzu.macos.Window.effectiveTheme

Checked type: `fn(Window)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L968).

```rinzu
fn effectiveTheme(self: Window) -> Int
```

## rinzu.macos.Window.hide

Checked type: `fn(Window)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L948).

```rinzu
fn hide(self: Window) -> Int
```

## rinzu.macos.Window.identity

Checked type: `fn(Window)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L844).

```rinzu
fn identity(self: Window) -> Int
```

## rinzu.macos.Window.menu

Checked type: `fn(Window,String)->Menu`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L869).

```rinzu
fn menu(self: Window, title: String) -> Menu
```

## rinzu.macos.Window.minimize

Checked type: `fn(Window)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L952).

```rinzu
fn minimize(self: Window) -> Int
```

## rinzu.macos.Window.openFile

Checked type: `fn(Window,String,String)->Future<Option<String>>`.

Error contract: `nothrow` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L885).

```rinzu
fn openFile(self: Window, title: String, directory: String) -> Future<Option<String>>
```

One file, as an owned absolute path. Cancel returns None. Empty directory uses AppKit's default.

## rinzu.macos.Window.restore

Checked type: `fn(Window)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L956).

```rinzu
fn restore(self: Window) -> Int
```

## rinzu.macos.Window.saveFile

Checked type: `fn(Window,String,String,String)->Future<Option<String>>`.

Error contract: `nothrow` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L890).

```rinzu
fn saveFile(self: Window, title: String, directory: String, name: String) -> Future<Option<String>>
```

Choose a destination; writing the file remains a separate operation.

## rinzu.macos.Window.setContentSize

Checked type: `fn(Window,Float,Float)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L857).

```rinzu
fn setContentSize(self: Window, width: Float, height: Float) -> Int
```

## rinzu.macos.Window.setMenu

Checked type: `fn(Window,Menu)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L876).

```rinzu
fn setMenu(self: Window, menu: Menu) -> Int
```

## rinzu.macos.Window.setMinimumContentSize

Checked type: `fn(Window,Float,Float)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L960).

```rinzu
fn setMinimumContentSize(self: Window, width: Float, height: Float) -> Int
```

## rinzu.macos.Window.setTheme

Checked type: `fn(Window,Int)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L964).

```rinzu
fn setTheme(self: Window, theme: Int) -> Int
```

## rinzu.macos.Window.setTitle

Checked type: `fn(Window,String)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L852).

```rinzu
fn setTitle(self: Window, title: String) -> Int
```

## rinzu.macos.Window.show

Checked type: `fn(Window)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L848).

```rinzu
fn show(self: Window) -> Int
```

## rinzu.macos.Window.state

Checked type: `fn(Window)->Int`.

Error contract: `throws(Error)` (inferred).

method · [source](../../sdk/macos/src/macos.rinzu#L944).

```rinzu
fn state(self: Window) -> Int
```

## rinzu.macos.WindowHandle

Checked type: `WindowHandle`.

opaque · [source](../../sdk/macos/src/macos.rinzu#L978).

```rinzu
opaque WindowHandle;
```

## rinzu.macos.actionRedo

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L2777).

```rinzu
let actionRedo = -2;
```

## rinzu.macos.actionUndo

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L2775).

```rinzu
let actionUndo = -1;
```

## rinzu.macos.actions

Checked type: `fn()->UiActions`.

Error contract: `nothrow` (inferred).

function · [source](../../sdk/macos/src/macos.rinzu#L3741).

```rinzu
fn actions() -> UiActions
```

## rinzu.macos.addressClamp

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L3132).

```rinzu
let addressClamp = 0;
```

## rinzu.macos.addressMirror

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L3136).

```rinzu
let addressMirror = 2;
```

## rinzu.macos.addressRepeat

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L3134).

```rinzu
let addressRepeat = 1;
```

## rinzu.macos.application

Checked type: `async(String,Float,Float,fn(UiApplication)->Future<Unit>)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/macos/src/macos.rinzu#L2469).

```rinzu
async fn application(title: String, width: Float, height: Float, body: fn (UiApplication) -> Future<Unit>) -> Int
```

Own the OS loop, Window, jobs and GPU resources; body runs entirely in Rinzu.

## rinzu.macos.applicationWithSignals

Checked type: `async(String,Float,Float,fn(UiApplication)->Future<Unit>)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/macos/src/macos.rinzu#L2474).

```rinzu
async fn applicationWithSignals(title: String, width: Float, height: Float, body: fn (UiApplication) -> Future<Unit>)
    -> Int
```

Opt into cooperative process signals, including rinzu dev cooperative restart.

## rinzu.macos.blendOpaque

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L3138).

```rinzu
let blendOpaque = 0;
```

## rinzu.macos.blendPremultiplied

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L3140).

```rinzu
let blendPremultiplied = 1;
```

## rinzu.macos.blendStraight

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L3142).

```rinzu
let blendStraight = 2;
```

## rinzu.macos.close

Checked type: `async(Session)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/macos/src/macos.rinzu#L1858).

```rinzu
async fn close(session: Session) -> Int
```

Stop and await run, then join Source children before close. Drain GPU work first.

## rinzu.macos.closeOwnerSessions

Checked type: `async()->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/macos/src/macos.rinzu#L1881).

```rinzu
async fn closeOwnerSessions() -> Int
```

Owner-scope recovery also closes native acquisitions not yet received by Source.

## rinzu.macos.column

Checked type: `fn(View,List<View>,Float,Float)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/macos/src/macos.rinzu#L2826).

```rinzu
fn column(parent: View, children: List<View>, padding: Float, gap: Float) -> Int
```

## rinzu.macos.commandCopy

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L2763).

```rinzu
let commandCopy = 2;
```

## rinzu.macos.commandCut

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L2761).

```rinzu
let commandCut = 1;
```

## rinzu.macos.commandPaste

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L2765).

```rinzu
let commandPaste = 3;
```

## rinzu.macos.commandQuit

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L2773).

```rinzu
let commandQuit = 7;
```

## rinzu.macos.commandRedo

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L2771).

```rinzu
let commandRedo = 6;
```

## rinzu.macos.commandSelectAll

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L2767).

```rinzu
let commandSelectAll = 4;
```

## rinzu.macos.commandUndo

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L2769).

```rinzu
let commandUndo = 5;
```

## rinzu.macos.compose2D

Checked type: `fn(Transform2D,Transform2D)->Transform2D`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/macos/src/macos.rinzu#L3185).

```rinzu
fn compose2D(first: Transform2D, next: Transform2D) -> Transform2D
```

Apply first, then next: next * first.

## rinzu.macos.dataActivated

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L4140).

```rinzu
let dataActivated = 2;
```

## rinzu.macos.dataCollapsed

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L4144).

```rinzu
let dataCollapsed = 4;
```

## rinzu.macos.dataExpanded

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L4142).

```rinzu
let dataExpanded = 3;
```

## rinzu.macos.dataSelected

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L4138).

```rinzu
let dataSelected = 1;
```

## rinzu.macos.dataSorted

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L4146).

```rinzu
let dataSorted = 5;
```

## rinzu.macos.diagnostic

Checked type: `fn()->String`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/macos/src/macos.rinzu#L1691).

```rinzu
fn diagnostic() -> String
```

Copy the latest Main-thread native diagnostic. Errors include this text.

## rinzu.macos.eventAction

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L1653).

```rinzu
let eventAction = 12;
```

## rinzu.macos.eventCloseRequested

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L1633).

```rinzu
let eventCloseRequested = 2;
```

## rinzu.macos.eventData

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L4136).

```rinzu
let eventData = 20;
```

## rinzu.macos.eventDraw

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L1637).

```rinzu
let eventDraw = 4;
```

## rinzu.macos.eventFocus

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L1661).

```rinzu
let eventFocus = 16;
```

## rinzu.macos.eventInputLimit

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L2492).

```rinzu
let eventInputLimit = 19;
```

## rinzu.macos.eventKeyDown

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L1641).

```rinzu
let eventKeyDown = 6;
```

## rinzu.macos.eventKeyUp

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L1643).

```rinzu
let eventKeyUp = 7;
```

## rinzu.macos.eventMouseDown

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L1645).

```rinzu
let eventMouseDown = 8;
```

## rinzu.macos.eventMouseMoved

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L1649).

```rinzu
let eventMouseMoved = 10;
```

## rinzu.macos.eventMouseUp

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L1647).

```rinzu
let eventMouseUp = 9;
```

## rinzu.macos.eventQuitRequested

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L1639).

```rinzu
let eventQuitRequested = 5;
```

## rinzu.macos.eventResized

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L1635).

```rinzu
let eventResized = 3;
```

## rinzu.macos.eventScroll

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L1659).

```rinzu
let eventScroll = 15;
```

## rinzu.macos.eventSelectionChanged

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L1657).

```rinzu
let eventSelectionChanged = 14;
```

## rinzu.macos.eventStarted

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L1631).

```rinzu
let eventStarted = 1;
```

## rinzu.macos.eventStopped

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L1651).

```rinzu
let eventStopped = 11;
```

## rinzu.macos.eventTaskFinished

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L1665).

```rinzu
let eventTaskFinished = 18;
```

## rinzu.macos.eventTextChanged

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L1655).

```rinzu
let eventTextChanged = 13;
```

## rinzu.macos.eventWindowState

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L1663).

```rinzu
let eventWindowState = 17;
```

## rinzu.macos.filterLinear

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L3130).

```rinzu
let filterLinear = 1;
```

## rinzu.macos.filterNearest

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L3128).

```rinzu
let filterNearest = 0;
```

## rinzu.macos.identity2D

Checked type: `fn()->Transform2D`.

Error contract: `nothrow` (inferred).

function · [source](../../sdk/macos/src/macos.rinzu#L3165).

```rinzu
fn identity2D() -> Transform2D
```

## rinzu.macos.metalAvailable

Checked type: `native()->Bool`.

native-function · [source](../../sdk/macos/src/macos.rinzu#L1415).

```rinzu
fn metalAvailable() -> CBool symbol "rinzu_macos_metal_available";
```

## rinzu.macos.modifierCommand

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L2759).

```rinzu
let modifierCommand = 1048576;
```

## rinzu.macos.modifierControl

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L2755).

```rinzu
let modifierControl = 262144;
```

## rinzu.macos.modifierOption

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L2757).

```rinzu
let modifierOption = 524288;
```

## rinzu.macos.modifierShift

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L2753).

```rinzu
let modifierShift = 131072;
```

## rinzu.macos.nextEvent

Checked type: `async(Session)->UiEvent`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/macos/src/macos.rinzu#L1759).

```rinzu
async fn nextEvent(session: Session) -> UiEvent
```

Wait for one owned event. A single reader may be active per Session.

## rinzu.macos.open

Checked type: `fn()->Session`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/macos/src/macos.rinzu#L1703).

```rinzu
fn open() -> Session
```

Open one Main-owned application resource scope. Close explicitly.

## rinzu.macos.pixelBGRA8

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L3120).

```rinzu
let pixelBGRA8 = 2;
```

## rinzu.macos.pixelRGBA8

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L3118).

```rinzu
let pixelRGBA8 = 1;
```

## rinzu.macos.readDocument

Checked type: `async(String)->String`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/macos/src/document_files.rinzu#L8).

```rinzu
async fn readDocument(path: String) -> String
```

Read plain UTF8 with a 1 MiB bound; the enclosing Owner recovers unreceived opens.

## rinzu.macos.rotation2D

Checked type: `fn(Float)->Transform2D`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/macos/src/macos.rinzu#L3178).

```rinzu
fn rotation2D(radians: Float) -> Transform2D
```

Positive rotation is clockwise in the batch's top-left coordinate system.

## rinzu.macos.row

Checked type: `fn(View,List<View>,Float,Float)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/macos/src/macos.rinzu#L2830).

```rinzu
fn row(parent: View, children: List<View>, padding: Float, gap: Float) -> Int
```

## rinzu.macos.run

Checked type: `async(Session,fn($rinzu_record_f7c713f12e29ebb74f67222ddd835128,Event)->Unit)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/macos/src/macos.rinzu#L1743).

```rinzu
async fn run(session: Session, handler: fn (RunContext, Event) -> Unit) -> Int
```

Run NSApplication on Main. The handler is synchronous and valid only during run.

## rinzu.macos.runEvents

Checked type: `fn(Session)->Future<Int>`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/macos/src/macos.rinzu#L1753).

```rinzu
fn runEvents(session: Session) -> Future<Int>
```

## rinzu.macos.saveDocument

Checked type: `async(String,String)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/macos/src/document_files.rinzu#L34).

```rinzu
async fn saveDocument(path: String, text: String) -> Int
```

Unique sibling temporary file -> bounded writes -> fsync -> atomic rename -> close.
Cancellation before installation removes the temporary during close/Owner recovery.

## rinzu.macos.scale2D

Checked type: `fn(Float,Float)->Transform2D`.

Error contract: `nothrow` (inferred).

function · [source](../../sdk/macos/src/macos.rinzu#L3173).

```rinzu
fn scale2D(x: Float, y: Float) -> Transform2D
```

## rinzu.macos.scrollPrecise

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L3043).

```rinzu
let scrollPrecise = 1;
```

## rinzu.macos.scrollViewport

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L3045).

```rinzu
let scrollViewport = 2;
```

## rinzu.macos.statistics

Checked type: `fn()->Statistics`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/macos/src/macos.rinzu#L1708).

```rinzu
fn statistics() -> Statistics
```

Measured live payloads and separately retained alias-check controls.

## rinzu.macos.stop

Checked type: `fn(Session)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/macos/src/macos.rinzu#L1830).

```rinzu
fn stop(session: Session) -> Int
```

## rinzu.macos.textCenter

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L3518).

```rinzu
let textCenter = 1;
```

## rinzu.macos.textComposing

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L3029).

```rinzu
let textComposing = 1;
```

Text snapshots with this flag are provisional IME input, outside document history.

## rinzu.macos.textLeft

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L3516).

```rinzu
let textLeft = 0;
```

## rinzu.macos.textRight

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L3520).

```rinzu
let textRight = 2;
```

## rinzu.macos.textureRenderTarget

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L3124).

```rinzu
let textureRenderTarget = 2;
```

## rinzu.macos.textureSampled

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L3122).

```rinzu
let textureSampled = 1;
```

## rinzu.macos.textureStorage

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L3126).

```rinzu
let textureStorage = 4;
```

## rinzu.macos.themeDark

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L4152).

```rinzu
let themeDark = 2;
```

## rinzu.macos.themeLight

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L4150).

```rinzu
let themeLight = 1;
```

## rinzu.macos.themeSystem

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L4148).

```rinzu
let themeSystem = 0;
```

## rinzu.macos.translation2D

Checked type: `fn(Float,Float)->Transform2D`.

Error contract: `nothrow` (inferred).

function · [source](../../sdk/macos/src/macos.rinzu#L3169).

```rinzu
fn translation2D(x: Float, y: Float) -> Transform2D
```

## rinzu.macos.wait

Checked type: `async(Command)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/macos/src/macos.rinzu#L1853).

```rinzu
async fn wait(command: Command) -> Int
```

Await and physically settle a submitted command. The handle becomes reusable.

## rinzu.macos.weightedColumn

Checked type: `fn(View,List<LayoutItem>,Float,Float)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/macos/src/macos.rinzu#L3815).

```rinzu
fn weightedColumn(parent: View, items: List<LayoutItem>, padding: Float, gap: Float) -> Int
```

## rinzu.macos.weightedRow

Checked type: `fn(View,List<LayoutItem>,Float,Float)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/macos/src/macos.rinzu#L3811).

```rinzu
fn weightedRow(parent: View, items: List<LayoutItem>, padding: Float, gap: Float) -> Int
```

## rinzu.macos.windowFullScreen

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L3041).

```rinzu
let windowFullScreen = 32;
```

## rinzu.macos.windowKey

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L3033).

```rinzu
let windowKey = 2;
```

## rinzu.macos.windowMain

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L3035).

```rinzu
let windowMain = 4;
```

## rinzu.macos.windowMinimized

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L3037).

```rinzu
let windowMinimized = 8;
```

## rinzu.macos.windowVisible

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L3031).

```rinzu
let windowVisible = 1;
```

## rinzu.macos.windowZoomed

Checked type: `Int`.

constant · [source](../../sdk/macos/src/macos.rinzu#L3039).

```rinzu
let windowZoomed = 16;
```

## rinzu.macos.withWindow

Checked type: `async(Session,String,Float,Float,fn(UiScope)->Future<Unit>)->Int`.

Error contract: `throws(Error)` (inferred).

function · [source](../../sdk/macos/src/macos.rinzu#L2328).

```rinzu
async fn withWindow(session: Session, title: String, width: Float, height: Float, body: fn (UiScope) -> Future<Unit>)
    -> Int
```

A Window scope within the running Application. The application has one event reader.
