use rinzu_compiler::{FileId, OperationKind, SourceFile, compile_source, verify_module};
fn compile(
    source: &str,
) -> Result<rinzu_compiler::FrontendArtifact, Vec<rinzu_compiler::Diagnostic>> {
    compile_source(&SourceFile::new(FileId(0), "capture.rinzu", source))
}
#[test]
fn captures_lower_to_exact_typed_native_plans_and_preserve_target_fallbacks() {
    let source = r#"
        native { fn invoke(handler:fn(Int)->Int)->Int symbol "invoke"; }
        class Editor {value:Int}
        fn edit(editor:Editor,value:Int)->Int fallback -7 {editor.value=editor.value+value;return editor.value;}
        fn main() {let editor=Editor(value:39);let result=invoke(fn(argument0:Int)->Int fallback -7 {return edit(editor, argument0);});}
    "#;
    let artifact = compile(source).unwrap();
    let callback = artifact
        .ir
        .functions
        .iter()
        .flat_map(|f| &f.blocks)
        .flat_map(|b| &b.operations)
        .find_map(|op| {
            if let OperationKind::PrepareNativeCall { callbacks, .. } = &op.kind {
                callbacks.first()
            } else {
                None
            }
        })
        .unwrap();
    assert!(callback.capture.is_some());
    assert!(callback.targets.iter().any(|target| {
        artifact
            .ir
            .functions
            .iter()
            .any(|f| f.name == *target && f.callback_fallback.is_some())
    }));
    let mut corrupted = artifact.ir.clone();
    for op in corrupted
        .functions
        .iter_mut()
        .flat_map(|f| &mut f.blocks)
        .flat_map(|b| &mut b.operations)
    {
        if let OperationKind::PrepareNativeCall { callbacks, .. } = &mut op.kind {
            callbacks[0].targets.reverse();
            callbacks[0].capture = None;
        }
    }
    assert!(verify_module(&corrupted).is_err());
}
#[test]
fn captured_worker_callback_and_buffer_result_are_rejected_before_backend() {
    for source in [
        "native {fn invoke(handler:fn(Int)->Int)->Int symbol \"invoke\";} fn edit(prefix:Int,value:Int)->Int {return prefix+value;} fn work()->Int {return invoke(makeCaptured0(39));} async fn main() {let result=await spawn work();}\nfn makeCaptured0(captured0:Int)->fn(Int)->Int {return fn(argument0:Int)->Int {return edit(captured0, argument0);};}\n",
        "native {fn invoke(handler:fn(Int)->String)->String symbol \"invoke\";} fn edit(prefix:String,value:Int)->String {return prefix;} fn main() {let value=invoke(makeCaptured0(\"x\"));}\nfn makeCaptured0(captured0:String)->fn(Int)->String {return fn(argument0:Int)->String {return edit(captured0, argument0);};}\n",
    ] {
        let diagnostics = compile(source).unwrap_err();
        assert!(
            diagnostics
                .iter()
                .any(|d| d.message.contains("WorkerSafe") || d.message.contains("Scalar or Unit")),
            "{diagnostics:?}"
        );
    }
}

#[test]
fn supplant_captures_keep_the_callback_signature_distinct_from_the_loop() {
    compile("native {supplant fn run(handler:fn(Int)->Unit)->Int symbol \"capture_run\";} class Editor {value:Int} fn event(editor:Editor,value:Int) {editor.value=value;} async fn main() {try{let loop=run;let frames=await loop(makeCaptured0(Editor(value:0)));}catch errors{}}\nfn makeCaptured0(captured0:Editor)->fn(Int)->Unit {return fn(argument0:Int)->Unit {event(captured0, argument0); return;};}\n").unwrap();
}
