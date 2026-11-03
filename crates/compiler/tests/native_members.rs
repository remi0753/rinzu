use rinzu_compiler::{DiagnosticCode, FileId, SourceFile, compile_source, parse_source};

fn file(text: &str) -> SourceFile {
    SourceFile::new(FileId(7), "native-members.rinzu", text)
}

#[test]
fn removed_userdata_declarations_are_rejected_without_reserving_the_identifier() {
    for declaration in [
        "class Context {const value:Int}",
        "enum Context {Ready}",
        "type Context={value:Int};",
        "native {opaque Context;}",
    ] {
        let text = format!("{declaration} native {{userdata Context;}} fn main(){{}}");
        let errors = compile_source(&file(&text)).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.code == DiagnosticCode::UnsupportedSafeFfi
                    && e.message.contains("userdata")
                    && e.suggestions
                        .iter()
                        .any(|s| s.message.contains("ordinary structural record"))),
            "{errors:#?}"
        );
    }
    compile_source(&file("class Data{const userdata:Int} fn userdata(userdata:Int)->Int{return userdata;} fn main(){let data=Data(userdata:1);let result=userdata(data.userdata);}")).unwrap();
    parse_source(&file(
        "native {fn userdata(value:Int)->Int symbol \"userdata\";}",
    ))
    .unwrap();
}

#[test]
fn native_types_and_aliases_cannot_own_any_method_form() {
    for declaration in ["opaque Owner;", "record Owner{value:Int}"] {
        for owner in ["Owner", "Alias", "Chained"] {
            for method in [
                "fn OWNER.read(self:OWNER)->Int{return 1;}",
                "private fn OWNER.read(self:OWNER)->Int{return 1;}",
                "fn OWNER.make()->Int{return 1;}",
                "async fn OWNER.read(self:OWNER)->Int{return 1;}",
                "private async fn OWNER.read(self:OWNER)->Int{return 1;}",
                "fn OWNER.read<T>(self:OWNER,value:T)->Int{return 1;}",
            ] {
                let text = format!(
                    "native{{{declaration}}}type Alias=Owner;type Chained=Alias;{} fn main(){{}}",
                    method.replace("OWNER", owner)
                );
                let errors = compile_source(&file(&text)).unwrap_err();
                assert!(
                    errors
                        .iter()
                        .any(|e| e.code == DiagnosticCode::UnsupportedSafeFfi
                            && e.message.contains("cannot own methods")
                            && e.suggestions.iter().any(|s| s.message.contains("class"))),
                    "{text}: {errors:#?}"
                );
                assert!(
                    errors.iter().all(|e| e.primary.file_id == FileId(7)
                        && e.primary.end_byte as usize <= text.len())
                );
            }
        }
    }
}

#[test]
fn native_member_blocks_accept_only_the_declared_scalar_fields() {
    for text in [
        "native{opaque Owner{const value:Int}}",
        "native{opaque Owner{fn read()->Int{return 1;}}}",
        "native{opaque Owner{class Nested{}}}",
        "native{record Owner{value:Int fn read()->Int{return 1;}}}",
        "native{record Owner{const value:Int}}",
        "native{record Owner{value:Int class Nested{}}}",
        "native{record Owner{value:Int callback:fn()->Int}}",
        "native{record Owner{value:Int}fn Owner.read()->Int symbol \"read\";}",
    ] {
        assert!(
            compile_source(&file(&format!("{text} fn main(){{}}"))).is_err(),
            "{text}"
        );
    }
}

#[test]
fn classes_and_free_functions_operate_on_native_references() {
    let text = r#"
native {
    opaque Handle;
    record Packet{value:Int}
    fn update(packet:Packet)->Int symbol "update";
    fn context(value:{owner:Owner})->Int symbol "context";
}
class Owner {
    const handle:Handle,
    const packet:Packet
    fn read(self:Owner)->Int{return self.packet.value;}
    fn update(self:Owner)->Int{return update(self.packet);}
}
fn read(packet:Packet)->Int{return packet.value;}
fn inspect(handle:Handle)->Int {
    let owner=Owner(handle:handle,packet:Packet(value:1));
    let status=owner.update();
    let result=context({owner=owner});
    return owner.read()+read(owner.packet)+result+status;
}
fn main(){}
"#;
    let artifact = compile_source(&file(text)).unwrap();
    rinzu_compiler::verify_module(&artifact.ir).unwrap();
    assert!(
        artifact
            .checked
            .source_native_signatures
            .iter()
            .any(|signature| matches!(
                &signature.parameters[..],
                [rinzu_compiler::SourceAbiType::UserData(_)]
            ))
    );
}
