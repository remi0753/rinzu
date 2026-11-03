use rinzu_compiler::{
    FileId, OperationKind, SourceFile, compile_source, format_source, verify_module,
};
fn source(text: &str) -> SourceFile {
    SourceFile::new(FileId(1), "state.rinzu", text)
}
#[test]
fn state_update_is_typed_verified_and_formatted() {
    let text = include_str!("../../codegen/tests/fixtures/m2_state.rinzu");
    let formatted = format_source(&source(text)).unwrap();
    assert_eq!(formatted, format_source(&source(&formatted)).unwrap());
    let artifact = compile_source(&source(&formatted)).unwrap();
    assert_eq!(artifact.ir.source_types["Editor"].kind, "state");
    let mut invalid = artifact.ir.clone();
    let op = invalid
        .functions
        .iter_mut()
        .flat_map(|f| &mut f.blocks)
        .flat_map(|b| &mut b.operations)
        .find(|op| matches!(op.kind, OperationKind::StoreField { .. }))
        .unwrap();
    if let OperationKind::StoreField { field, .. } = &mut op.kind {
        *field = 999;
    }
    assert!(verify_module(&invalid).is_err());
}
#[test]
fn state_properties_and_immutable_record_boundary_are_checked() {
    for text in [
        "class Data {const n:Int} fn main() {let d=Data(n:0);d.n=1;}",
        "class Data {n:Int} fn main() {let d=Data(n:0);let copy=d with {n:1};}",
        "class Data {n:Int} fn main() {let d=Data(n:0);d.n=true;}",
        "class Data {n:Int} fn main() {let d=Data(n:0);d.m=1;}",
        "class Data {n:Int} fn main() {let d=Data(n:0);let same=d==d;}",
        "class Data {n:Int} fn main() {let d=Data(n:0);let index={d:1};}",
        "class Data {n:Int} fn use(d:Data) {} async fn main() {let pending=spawn use(Data(n:0));await pending;}",
        "class Data {n:Int} let d=Data(n:0);fn main() {}",
    ] {
        assert!(compile_source(&source(text)).is_err(), "{text}");
    }
    compile_source(&source("enum Event {state(value:Int)} fn state(state:Int)->Int {return state;} fn main() {let state=state(1);let event=Event.state(value:state);}" )).unwrap();
    compile_source(&source(
        "class Box<T> {value:T} fn main() {let box=Box<Int>(value:0);box.value=9;}",
    ))
    .unwrap();
}
