use rinzu_compiler::{FileId, OperationKind, SourceFile, compile_source, verify_module};
#[test]
fn typed_control_is_main_only_and_verified() {
    let good = "async fn ready()->Int{return 42;} async fn main(){let f=ready();let r=futureReady(f);let c=cancelFuture(f);let t=timeoutFuture(f);let q=requestShutdown();}";
    let artifact = compile_source(&SourceFile::new(FileId(0), "control.rinzu", good)).unwrap();
    let mut invalid_entry = artifact.ir.clone();
    invalid_entry
        .functions
        .iter_mut()
        .find(|f| f.name == "main")
        .unwrap()
        .facts
        .requires_owner = false;
    assert!(verify_module(&invalid_entry).is_err());
    compile_source(&SourceFile::new(
        FileId(0),
        "sync-control.rinzu",
        "fn main(){let q=requestShutdown();}",
    ))
    .unwrap();
    let mut broken = artifact.ir.clone();
    for op in broken
        .functions
        .iter_mut()
        .flat_map(|f| &mut f.blocks)
        .flat_map(|b| &mut b.operations)
    {
        if let OperationKind::TaskControl {
            future: Some(_), ..
        } = &mut op.kind
        {
            if let OperationKind::TaskControl { future, .. } = &mut op.kind {
                *future = None;
            }
            break;
        }
    }
    assert!(verify_module(&broken).is_err());
    for source in [
        "async fn main(){let value=cancelFuture(42);}",
        "fn bad()->Bool{return requestShutdown();} async fn main(){let f=spawn bad();}",
    ] {
        assert!(
            compile_source(&SourceFile::new(FileId(0), "bad.rinzu", source)).is_err(),
            "{source}"
        );
    }
}
