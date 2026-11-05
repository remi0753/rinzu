use rinzu_compiler::{DiagnosticCode, FileId, SourceFile, compile_source, format_source};
fn file(source: &str) -> SourceFile {
    SourceFile::new(FileId(702), "interfaces.rinzu", source)
}
fn ok(source: &str) {
    compile_source(&file(source)).unwrap_or_else(|e| panic!("{source}\n{e:#?}"));
    let formatted = format_source(&file(source)).unwrap();
    assert_eq!(formatted, format_source(&file(&formatted)).unwrap());
    compile_source(&file(&formatted)).unwrap();
}
fn invalid(source: &str) {
    let errors = compile_source(&file(source)).expect_err(source);
    assert!(
        errors.iter().any(|e| matches!(
            e.code,
            DiagnosticCode::InvalidInterface | DiagnosticCode::InvalidErrorContract
        )),
        "{errors:#?}"
    );
}
const READERS: &str = r#"
import rinzu.list;
import rinzu.map;
error ReadFailed{}
interface Reader{fn read(self:Self)->String throws ReadFailed;}
class MemoryReader interface Reader{text:String fn read(self:MemoryReader)->String nothrow{return self.text;}}
class CheckedReader interface Reader{text:String fn read(self:CheckedReader)->String throws ReadFailed{if self.text==""{throw ReadFailed(message:"empty input");}return self.text;}}
"#;

#[test]
fn concrete_calls_use_the_method_contract_and_constrained_calls_use_the_interface() {
    ok(&format!(
        "{READERS}fn text<R interface Reader>(reader:R)->String nothrow{{return reader.text;}}fn main(){{let value=text(MemoryReader(text:\"ok\"));}}"
    ));
    ok(&format!(
        "{READERS} fn through<R interface Reader>(reader:R)->String throws ReadFailed{{return reader.read();}}fn main(){{let memory=MemoryReader(text:\"ok\");let direct=memory.read();try{{let a=through(memory);let b=through(CheckedReader(text:\"ok\"));}}catch problems{{}}}}"
    ));
    invalid(&format!(
        "{READERS}fn invalid<R interface Reader>(reader:R)->String nothrow{{return reader.read();}}fn main(){{let value=invalid(MemoryReader(text:\"ok\"));}}"
    ));
    ok(&format!(
        "{READERS}fn recover<R interface Reader>(reader:R)->String nothrow{{try{{return reader.read();}}catch problems{{return \"fallback\";}}}}fn main(){{let s=recover(MemoryReader(text:\"ok\"));}}"
    ));
}

#[test]
fn self_arguments_results_and_enum_payloads_use_the_concrete_owner() {
    ok(
        r#"interface Copyable{fn copy(self:Self)->Self nothrow;}class Counter interface Copyable{value:Int fn copy(self:Counter)->Counter nothrow{return Counter(value:self.value);}}fn clone<C interface Copyable>(value:C)->C nothrow{return value.copy();}fn main(){let c=clone(Counter(value:42));}"#,
    );
    ok(
        r#"interface Labelled{fn label(self:Self)->String nothrow;}enum Entry interface Labelled{Empty,Named(text:String),fn label(self:Entry)->String nothrow{switch self{case Entry.Empty:{return "empty";}case Entry.Named(text):{return text;}}}}fn label<T interface Labelled>(value:T)->String nothrow{return value.label();}fn main(){let text=label(Entry.Named(text:"ok"));}"#,
    );
}

#[test]
fn generic_owners_interfaces_aliases_and_multiple_constraints_compose() {
    ok(
        r#"interface Getter<T>{fn get(self:Self)->T nothrow;}class Box<T> interface Getter<T>{value:T fn get(self:Box<T>)->T nothrow{return self.value;}}fn unbox<T,B interface Getter<T>>(box:B,otherwise:T)->T nothrow{return box.get();}fn main(){let n=unbox(Box(value:42),0);}"#,
    );
    ok(
        r#"interface Labelled{fn label(self:Self)->String nothrow;}enum Maybe<T interface Labelled> interface Labelled{None,Some(value:T),fn label(self:Maybe<T>)->String nothrow{switch self{case Maybe.None:{return "none";}case Maybe.Some(value):{return value.label();}}}}class Text interface Labelled{value:String fn label(self:Text)->String nothrow{return self.value;}}fn main(){let m=Maybe<Text>.Some(value:Text(value:"ok"));let s=m.label();}"#,
    );
    ok(
        r#"interface A{fn get(self:Self)->Int throws Error;}interface B{fn get(self:Self)->Int nothrow;}class C interface A+B{fn get(self:C)->Int nothrow{return 42;}}fn read<T:Equality interface A+B>(value:T)->Int nothrow{return value.get();}fn main(){let n=read(C());}"#,
    );
}

#[test]
fn nominal_conformance_missing_methods_signatures_and_effects_are_checked() {
    for source in [
        "interface I{fn f(self:Self)nothrow;}class C interface I{}fn main(){}",
        "interface I{fn f(self:Self)->Int nothrow;}class C interface I{fn f(self:C)->String nothrow{return \"bad\";}}fn main(){}",
        "error E{}interface I{fn f(self:Self)nothrow;}class C interface I{fn f(self:C)throws E{throw E(message:\"bad\");}}fn main(){}",
        "interface I{fn f(self:Self)nothrow;}class C{fn f(self:C)nothrow{}}fn use<T interface I>(v:T){v.f();}fn main(){use(C());}",
        "interface I{fn f(self:Self)nothrow;}class C interface I{private fn f(self:C)nothrow{}}fn main(){}",
        "class C{}fn use<T interface C>(v:T){}fn main(){}",
        "interface I{fn f(self:Self)->Unknown nothrow;}fn main(){}",
        "interface I{fn f(self:Self)throws String;}fn main(){}",
        "interface I{fn f(self:Self)throws Error,Error;}fn main(){}",
        "error E{}interface I{fn f(self:Self)nothrow;}class C interface I{fn f(self:C){throw E(message:\"bad\");}}fn main(){}",
    ] {
        invalid(source);
    }
    assert!(compile_source(&file("interface I{fn f(self:Self);}fn main(){}")).is_err());
    assert!(compile_source(&file("interface I{}fn f(v:I){}fn main(){}")).is_err());
    assert!(
        compile_source(&file(
            "native{opaque Handle interface I;}interface I{}fn main(){}"
        ))
        .is_err()
    );
}

#[test]
fn interface_contracts_follow_symbolic_values_through_containers_fields_and_closures() {
    for body in [
        "let item={reader=r};return item.reader.read();",
        "let values=[r];for value in values{return value.read();}return \"empty\";",
        "let item=rinzu.list.get([r],0);switch item{case Option.Some(value):{return value.read();}case Option.None:{return \"empty\";}}",
        "let item=rinzu.map.get({\"key\":r},\"key\");switch item{case Option.Some(value):{return value.read();}case Option.None:{return \"empty\";}}",
        "let get=fn()->R nothrow{return r;};let value=get();return value.read();",
        "let read=fn()->String nothrow{return r.read();};return read();",
        "let value=identity(r);return value.read();",
        "let id=identity<R>;let value=id(r);return value.read();",
        "let read=R.read;return read(r);",
        "return R.read(r);",
    ] {
        invalid(&format!(
            "{READERS}fn identity<T>(v:T)->T nothrow{{return v;}}fn read<R interface Reader>(r:R)->String nothrow{{{body}}}fn main(){{let value=read(MemoryReader(text:\"ok\"));}}"
        ));
    }
}

#[test]
fn constraints_on_interface_arguments_and_generic_implementation_bodies_are_enforced() {
    ok(
        "interface I{}class V interface I{}interface Wrap<T interface I>{fn get(self:Self)->T nothrow;}class Box<T interface I> interface Wrap<T>{value:T fn get(self:Box<T>)->T nothrow{return self.value;}}fn main(){let box=Box(value:V());}",
    );
    assert!(
        compile_source(&file(
            "interface I<T:Key>{}class V{value:Float}class C interface I<V>{}fn main(){}"
        ))
        .is_err()
    );
    assert!(
        compile_source(&file(
            "interface I<T>{}class C interface I<Unknown>{}fn main(){}"
        ))
        .is_err()
    );
    invalid(&format!(
        "{READERS}interface ReadBox{{fn read(self:Self)->String nothrow;}}class Box<R interface Reader> interface ReadBox{{value:R fn read(self:Box<R>)->String nothrow{{return self.value.read();}}}}fn main(){{let b=Box(value:MemoryReader(text:\"ok\"));}}"
    ));
}

#[test]
fn async_interface_contract_is_latent_until_await_and_survives_capture() {
    let declarations = "error ReadFailed{}interface Reader{async fn read(self:Self)->String throws ReadFailed;}class Memory interface Reader{const text:String async fn read(self:Memory)->String nothrow{return self.text;}}";
    ok(&format!("{declarations}fn start<R interface Reader>(reader:R)->Future<String> nothrow{{return reader.read();}}async fn main(){{let direct=await Memory(text:\"ok\").read();try{{let text=await start(Memory(text:\"ok\"));}}catch problems{{}}}}" ).replace("let direct=await Memory(text:\"ok\").read();", "let memory=Memory(text:\"ok\");let direct=await memory.read();"));
    invalid(&format!(
        "{declarations}async fn read<R interface Reader>(reader:R)->String nothrow{{return await reader.read();}}async fn main(){{let text=await read(Memory(text:\"ok\"));}}"
    ));
    assert!(compile_source(&file(&format!("{declarations}fn start<R interface Reader>(reader:R)->Future<String> nothrow{{return reader.read();}}fn main(){{let memory=Memory(text:\"ok\");let pending=start(memory);}}"))).is_err());
}

#[test]
fn worker_results_retain_symbolic_interface_bounds_after_await() {
    let declarations = READERS.replace(
        "class MemoryReader interface Reader{text:String",
        "class MemoryReader interface Reader{const text:String",
    );
    invalid(&format!(
        "{declarations}fn identity<T:Transferable>(value:T)->T nothrow{{return value;}}async fn read<R:Transferable interface Reader>(reader:R)->String nothrow{{let pending=spawn identity(reader);let value=await pending;return value.read();}}async fn main(){{let text=await read(MemoryReader(text:\"ok\"));}}"
    ));
}
