use rinzu_compiler::{FileId, SourceFile, compile_source, verify_module};

#[test]
fn xor_and_exit_status_have_concrete_types_and_main_effect() {
    let file = |text| SourceFile::new(FileId(1), "cli.rinzu", text);
    let good = "async fn main(){let x=bytesXor(byteArray(b\"\\x00\\xff\"),CUInt8(255));let accepted=setProcessExitStatus(7);}";
    let artifact = compile_source(&file(good)).unwrap();
    assert!(verify_module(&artifact.ir).is_ok());
    for bad in [
        "fn main(){let x=bytesXor(b\"A\",CUInt8(1));}",
        "fn main(){let mask=1;let x=bytesXor(byteArray(b\"A\"),mask);}",
        "async fn main(){let q=setProcessExitStatus(CInt(1));}",
        "fn status()->Bool{return setProcessExitStatus(1);} async fn main(){let q=spawn status();}",
    ] {
        assert!(compile_source(&file(bad)).is_err(), "{bad}");
    }
    compile_source(&file("fn main(){let q=setProcessExitStatus(1);}")).unwrap();
    let mut broken = artifact.ir.clone();
    for operation in broken
        .functions
        .iter_mut()
        .flat_map(|f| &mut f.blocks)
        .flat_map(|b| &mut b.operations)
    {
        if let rinzu_compiler::OperationKind::TaskControl { future, .. } = &mut operation.kind {
            *future = None;
        }
    }
    assert!(verify_module(&broken).is_err());
}
