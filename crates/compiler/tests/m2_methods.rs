use rinzu_compiler::{
    FileId, OperationKind, SourceFile, compile_source, format_source, verify_module,
};
fn source(text: &str) -> SourceFile {
    SourceFile::new(FileId(1), "methods.rinzu", text)
}

#[test]
fn methods_normalize_before_closures_and_use_existing_verified_ir() {
    let text = include_str!("../../../tests/fixtures/methods/editor.rinzu");
    let formatted = format_source(&source(text)).unwrap();
    assert_eq!(formatted, format_source(&source(&formatted)).unwrap());
    let artifact = compile_source(&source(&formatted)).unwrap();
    let calls = artifact
        .ir
        .functions
        .iter()
        .flat_map(|f| &f.blocks)
        .flat_map(|b| &b.operations)
        .filter_map(|op| {
            if let OperationKind::ManagedCall {
                target, arguments, ..
            } = &op.kind
            {
                Some((target, arguments.len()))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert!(
        calls
            .iter()
            .any(|(name, n)| name.as_str() == "Editor.replace" && *n == 2),
        "{calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|(name, n)| name.as_str() == "Editor.snapshot" && *n == 1),
        "{calls:?}"
    );
    assert!(
        artifact
            .ir
            .source_types
            .keys()
            .any(|n| n.starts_with("$rinzu_closure_"))
    );
    assert!(
        artifact
            .checked
            .functions
            .iter()
            .all(|f| f.facts.worker_safe)
    );
    let mut invalid = artifact.ir.clone();
    for op in invalid
        .functions
        .iter_mut()
        .flat_map(|f| &mut f.blocks)
        .flat_map(|b| &mut b.operations)
    {
        if let OperationKind::ManagedCall {
            target, arguments, ..
        } = &mut op.kind
            && target == "Editor.replace"
        {
            arguments.remove(0);
            break;
        }
    }
    assert!(verify_module(&invalid).is_err());
}

#[test]
fn nominal_owner_kinds_static_instance_and_free_self_work() {
    for text in [
        "class Data {const value:Int\n    fn add(self:Data,n:Int)->Data {return Data(value:self.value+n);}\n}  fn main(){let d=Data(value:1);let result=d.add(2);}",
        "enum Event {Idle,Value(value:Int)} fn Event.read(self:Event)->Int {switch self {case Event.Idle: {return 0;} case Event.Value(value): {return value;}}} fn main(){let e=Event.Value(value:42);let n=e.read();}",
        "native {opaque Token;fn token()->Token symbol \"token\";} class TokenOwner {const handle:Token fn identity(self:TokenOwner)->Token {return self.handle;}} fn main(){try{let t=TokenOwner(handle:token());let copy=t.identity();}catch errors{}}",
        "native {record Input {value:CInt}} class InputBox {const input:Input fn read(self:InputBox)->CInt {return self.input.value;}} fn main(){try{let input=Input(value:CInt(1));let box=InputBox(input:input);let value=box.read();}catch errors{}}",
        "class Editor {value:Int\n    fn create()->Editor {return Editor(value:0);}\n}  fn free(self:Int)->Int {return self;} fn main(){let editor=Editor.create();let self=free(1);}",
        "class Counter {const value:CUInt8\n    fn read(self:Counter)->CUInt8 {return self.value;}\n}  fn main(){let c=Counter(value:CUInt8(1));let value=2+c.read();let same=1==c.read();}",
    ] {
        compile_source(&source(text)).unwrap_or_else(|errors| panic!("{text}\n{errors:#?}"));
    }
}

#[test]
fn invalid_method_surfaces_report_source_ranges() {
    for (text, message) in [
        (
            "fn Int.read(self:Int)->Int {return self;} fn main(){}",
            "same module",
        ),
        (
            "class Editor {value:Int\n    fn read(self:Int)->Int {return self;}\n}  fn main(){}",
            "first parameter",
        ),
        (
            "class Editor {value:Int\n    fn read(n:Int,self:Editor){}\n}  fn main(){}",
            "first parameter",
        ),
        (
            "class Editor {read:fn()->Int\n    fn read(self:Editor)->Int{return 1;}\n}  fn main(){}",
            "conflicts",
        ),
        (
            "enum Event {Idle} fn Event.Idle()->Event {return Event.Idle();} fn main(){}",
            "conflicts",
        ),
        (
            "class Box<T> {value:T\n    fn read(self:Box<Int>)->Int {return self.value;}\n}  fn main(){}",
            "generic",
        ),
        (
            "class Editor {value:Int\n    fn read<T>(self:Editor,value:T)->T {return value;}\n}  fn main(){}",
            "generic",
        ),
        (
            "class Editor {value:Int\n    fn create()->Editor {return Editor(value:0);}\n}  fn main(){let e=Editor(value:1);let copy=e.create();}",
            "static method",
        ),
        (
            "class Editor {value:Int\n    fn read(self:Editor)->Int{return self.value;}\n}  fn main(){let e=Editor(value:1);let f=e.read;}",
            "explicit call or fn expression",
        ),
        (
            "class Editor {value:Int\n    fn read(self:Editor)->Int{return self.value;}\n}  fn main(){let e=Editor(value:1);let n=e.read(self:e);}",
            "positional",
        ),
        (
            "class Data {const value:Int\n    fn read(self:Data)->Int{return self.value;}\n}  async fn main(){let d=Data(value:1);await spawn d.read();}",
            "explicit qualified",
        ),
        (
            "class Data {const value:Int\n    fn update(self:Data){self.value=1;}\n}  fn main(){}",
            "const field",
        ),
    ] {
        let errors = compile_source(&source(text)).unwrap_err();
        assert!(
            errors.iter().any(|e| e.message.contains(message)),
            "{text}\n{errors:#?}"
        );
        assert!(
            errors.iter().all(
                |e| e.primary.file_id == FileId(1) && e.primary.end_byte as usize <= text.len()
            )
        );
    }
}

#[test]
fn worker_transfer_async_and_borrow_rules_are_the_ordinary_function_rules() {
    for (text, message) in [
        (
            "class Editor {value:Int\n    fn read(self:Editor)->Int{return self.value;}\n}  async fn main(){await spawn Editor.read(Editor(value:1));}",
            "not Transferable",
        ),
        (
            "class Editor {value:Int\n    async fn read(self:Editor)->Int{return self.value;}\n}  fn work()->Int {let e=Editor(value:1);let f=e.read();return 0;} async fn main(){await spawn work();}",
            "WorkerSafe",
        ),
        (
            "native {opaque Token;fn token()->Token symbol \"token\";} class TokenOwner {const handle:Token fn read(self:TokenOwner)->Int{return 1;}} async fn main(){await spawn TokenOwner.read(TokenOwner(handle:token()));}",
            "not Transferable",
        ),
    ] {
        let errors = compile_source(&source(text)).unwrap_err();
        assert!(
            errors.iter().any(|e| e.message.contains(message)),
            "{errors:#?}"
        );
    }
    compile_source(&source("class Editor {value:Int\n    fn update(self:Editor,n:Int)->Int {self.value=n;return self.value;}\n}  fn work()->Int{let e=Editor(value:0);return e.update(42);} async fn main(){let result=await spawn work();}")).unwrap();
}

#[test]
fn bound_method_cannot_capture_a_borrowed_field() {
    use rinzu_compiler::{CompilerOptions, ExternalType, compile_source_with_options};
    let options = CompilerOptions {
        external_types: [("View".into(), ExternalType { call_only: true })]
            .into_iter()
            .collect(),
        owner_sync_handlers: ["Envelope.read".into(), "handler".into()]
            .into_iter()
            .collect(),
        ..Default::default()
    };
    let text = "class Envelope {const view:View\n    fn read(self:Envelope,value:Int)->Int {return value;}\n}  fn handler(envelope:Envelope){let f=fn(argument0:Int)->Int {return Envelope.read(envelope, argument0);};} fn main(){}";
    let errors = compile_source_with_options(&source(text), &options).unwrap_err();
    assert!(
        errors.iter().any(
            |e| e.message.contains("capture `envelope`") && e.message.contains("envelope.view")
        ),
        "{errors:#?}"
    );
}

#[test]
fn receiver_sugar_has_the_same_effect_calls_and_allocations_as_free_functions() {
    use std::collections::BTreeMap;
    let free = "class Editor {value:Int} fn create()->Editor{return Editor(value:40);} fn add(editor:Editor,n:Int)->Int{editor.value=editor.value+n;return editor.value;} fn read(editor:Editor)->Int{return editor.value;} fn main(){let editor=create();let changed=add(editor,2);let readValue=read(editor);let field=editor.value;}";
    let methods = "class Editor {value:Int fn create()->Editor{return Editor(value:40);} fn add(self:Editor,n:Int)->Int{self.value=self.value+n;return self.value;} fn read(self:Editor)->Int{return self.value;}} fn main(){let editor=Editor.create();let changed=editor.add(2);let readValue=editor.read();let field=editor.value;}";
    let free = compile_source(&source(free)).unwrap();
    let methods = compile_source(&source(methods)).unwrap();
    let name = |name: &str| name.strip_prefix("Editor.").unwrap_or(name).to_owned();
    let facts = |a: &rinzu_compiler::FrontendArtifact| {
        a.checked
            .functions
            .iter()
            .map(|f| (name(&f.declaration.name), f.facts.clone()))
            .collect::<BTreeMap<_, _>>()
    };
    assert_eq!(facts(&free), facts(&methods));
    let operations = |a: &rinzu_compiler::FrontendArtifact| {
        a.ir.functions
            .iter()
            .map(|f| {
                let mut calls = Vec::new();
                let mut allocations = 0;
                let mut polls = 0;
                for op in f.blocks.iter().flat_map(|b| &b.operations) {
                    match &op.kind {
                        OperationKind::ManagedCall {
                            target, arguments, ..
                        } => calls.push((name(target), arguments.len())),
                        OperationKind::Construct { .. } => allocations += 1,
                        OperationKind::GcSafepoint { .. } => polls += 1,
                        _ => {}
                    }
                }
                (name(&f.name), (calls, allocations, polls))
            })
            .collect::<BTreeMap<_, _>>()
    };
    assert_eq!(operations(&free), operations(&methods));
}
