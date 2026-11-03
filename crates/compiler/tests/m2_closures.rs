use rinzu_compiler::{
    FileId, OperationKind, SourceFile, compile_source, format_source, verify_module,
};
fn source(text: &str) -> SourceFile {
    SourceFile::new(FileId(1), "closures.rinzu", text)
}

#[test]
fn empty_callable_containers_lower_to_uninhabited_dispatch_without_dummy_targets() {
    for text in [
        "class Layer {const apply:fn(Int)->Int} fn invoke(layer:Layer)->Int{return layer.apply(42);} fn main(){let layers:List<Layer> = [];switch listGet(layers,0){case Option.Some(layer):{let value=invoke(layer);}case Option.None:{}}}",
        "class Layer {const apply:fn(Int)->Future<Int>} fn invoke(layer:Layer)->Future<Int>{return layer.apply(42);} async fn main(){let layers:List<Layer> = [];switch listGet(layers,0){case Option.Some(layer):{try{let value=await invoke(layer);}catch errors{}}case Option.None:{}}}",
    ] {
        let artifact =
            compile_source(&source(text)).unwrap_or_else(|errors| panic!("{text}\n{errors:#?}"));
        verify_module(&artifact.ir).unwrap();
        assert!(artifact.ir.functions.iter().flat_map(|function| &function.blocks)
            .any(|block| matches!(&block.terminator, rinzu_compiler::Terminator::Switch{cases,..} if cases.is_empty())));
    }
    let errors = compile_source(&source(
        "enum Present {value} fn consume(value:Present)->Int {switch value{}} fn main(){}",
    ))
    .unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.code == rinzu_compiler::DiagnosticCode::NonExhaustiveSwitch),
        "{errors:#?}"
    );
}
#[test]
fn lambda_fields_results_generic_and_cycles_lower_to_typed_closed_dispatch() {
    for text in [
        "fn identity(x:Int)->Int {return x;} class Command {const run:fn(Int)->Int} fn make()->fn(Int)->Int {return identity;} fn main() {let command=Command(run:make());let value=command.run(42);}",
        "fn add(prefix:Int,value:Int)->Int {return prefix+value;} fn make()->fn(Int)->Int {return makeCaptured0(40);} fn main() {let function=make();let value=function(2);}\nfn makeCaptured0(captured0:Int)->fn(Int)->Int {return fn(argument0:Int)->Int {return add(captured0, argument0);};}\n",
        "fn choose<T>(value:T,ignored:Int)->T {return value;} fn main() {let function=makeCaptured0(\"typed\");let value=function(2);}\nfn makeCaptured0(captured0:String)->fn(Int)->String {return fn(argument0:Int)->String {return choose<String>(captured0, argument0);};}\n",
        "class Editor {value:Int,command:Option<fn(Int)->Int>} fn edit(editor:Editor,value:Int)->Int {editor.value=value;return editor.value;} fn main() {let editor=Editor(value:0,command:Option<fn(Int)->Int>.None());editor.command=Option<fn(Int)->Int>.Some(value:fn(argument0:Int)->Int {return edit(editor, argument0);});switch editor.command {case Option.None: {} case Option.Some(function): {let value=function(42);}}}",
    ] {
        let artifact = compile_source(&source(text)).unwrap_or_else(|e| panic!("{text}\n{e:#?}"));
        assert!(
            artifact
                .ir
                .source_types
                .keys()
                .any(|name| name.starts_with("$rinzu_closure_"))
        );
        let formatted = format_source(&source(text)).unwrap();
        assert_eq!(formatted, format_source(&source(&formatted)).unwrap());
        compile_source(&source(&formatted)).unwrap();
        let mut invalid = artifact.ir.clone();
        let op = invalid
            .functions
            .iter_mut()
            .flat_map(|f| &mut f.blocks)
            .flat_map(|b| &mut b.operations)
            .find(|o| matches!(&o.kind, OperationKind::Construct { arguments,.. } if !arguments.is_empty()))
            .unwrap();
        if let OperationKind::Construct { arguments, .. } = &mut op.kind {
            arguments.clear();
        }
        assert!(verify_module(&invalid).is_err());
    }
}
#[test]
fn invalid_capture_signature_and_domain_are_rejected_at_source() {
    for text in [
        "fn add(x:Int,y:Int)->Int {return x+y;} fn main() {let f=fn(argument0:Int)->Int {return add(true, argument0);};}",
        "fn add(x:Int)->Int {return x;} fn main() {let f=makeCaptured0(1, 2);}\nfn makeCaptured0(captured0:Int, captured1:Int)->fn()->Int {return fn()->Int {return add(captured0, captured1);};}\n",
        "native {fn external(x:Int)->Int symbol \"external\";} fn main() {let f:fn(Int)->Int=fn()->Int{return external(1);};}",
        "fn add(x:Int)->Int {return x;} fn consume(f:fn()->Int) {} async fn main() {await spawn consume(makeCaptured0(1));}\nfn makeCaptured0(captured0:Int)->fn()->Int {return fn()->Int {return add(captured0);};}\n",
        "error Failed{} async fn later()->Int {throw Failed(message:\"failure\");} fn consume(pending:Future<Int>,value:Int)->Int {return value;} async fn main() {let pending=later();let f=fn(argument0:Int)->Int {return consume(pending, argument0);};}",
    ] {
        let errors = compile_source(&source(text)).unwrap_err();
        assert!(
            errors.iter().any(|e| e.primary.file_id == FileId(1)),
            "{errors:?}"
        );
    }
}

#[test]
fn call_only_capture_is_rejected_but_native_record_references_can_be_captured() {
    use rinzu_compiler::{CompilerOptions, ExternalType, compile_source_with_options};
    let options = CompilerOptions {
        external_types: [("View".into(), ExternalType { call_only: true })]
            .into_iter()
            .collect(),
        owner_sync_handlers: ["consume".into(), "owner".into()].into_iter().collect(),
        ..Default::default()
    };
    let text = "class Envelope {const view:View} fn consume(envelope:Envelope,value:Int)->Int {return value;} fn owner(envelope:Envelope) {let function=fn(argument0:Int)->Int {return consume(envelope, argument0);};} fn main() {}";
    let errors = compile_source_with_options(&source(text), &options).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("capture `envelope`")
                && error.message.contains("envelope.view")
                && error.primary.file_id == FileId(1)),
        "{errors:#?}"
    );
    let options = CompilerOptions {
        sdk_files: [FileId(1)].into_iter().collect(),
        ..Default::default()
    };
    let text = "native {record Input {value:CInt}} fn consume(input:Input,value:Int)->Int {return value;} fn main() {let input=Input(value:CInt(1));let function=fn(argument0:Int)->Int {return consume(input, argument0);};let value=function(42);}";
    compile_source_with_options(&source(text), &options).unwrap();
}

#[test]
fn rejected_indirect_worker_effect_names_original_field_and_call_span() {
    let text = "class Command {const run:fn(Int)->Int} fn identity(value:Int)->Int {return value;} async fn later() {} fn unsafeTarget(value:Int)->Int {let pending=later();return value;} fn work()->Int {let command=Command(run:identity);return command.run(42);} async fn main() {await spawn work();}";
    let errors = compile_source(&source(text)).unwrap_err();
    let error = errors
        .iter()
        .find(|e| e.message.contains("field path: Command.run"))
        .unwrap();
    assert!(error.message.contains("unsafeTarget"), "{errors:#?}");
    assert!(error.secondary.iter().any(|(range, _)| &text
        [range.start_byte as usize..range.end_byte as usize]
        == "command.run(42)"));
}
