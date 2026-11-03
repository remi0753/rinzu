use rinzu_compiler::{
    FileId, OperationKind, SourceFile, compile_source, format_source, verify_module,
};

fn source(text: &str) -> SourceFile {
    SourceFile::new(FileId(1), "classes.rinzu", text)
}

#[test]
fn class_methods_const_and_construction_share_verified_aggregate_ir() {
    let text = r#"
class Counter {
    const limit:Int,
    value:Int
    fn increase(self:Counter, amount:Int) { self.value = self.value + amount; }
    fn read(self:Counter)->Int { return self.value; }
    fn create()->Counter { return Counter(limit:100, value:0); }
}
fn main() {
    let counter = Counter.create();
    let alias = counter;
    alias.increase(2);
    let result = counter.read();
}
"#;
    let formatted = format_source(&source(text)).unwrap();
    assert_eq!(formatted, format_source(&source(&formatted)).unwrap());
    let artifact = compile_source(&source(&formatted)).unwrap();
    assert_eq!(artifact.ir.source_types["Counter"].const_fields, [0]);
    assert_eq!(artifact.ir.source_types["Counter"].fields.len(), 2);
    assert!(
        artifact
            .ir
            .functions
            .iter()
            .any(|f| f.name == "Counter.increase")
    );
    let mut invalid = artifact.ir.clone();
    let operation = invalid
        .functions
        .iter_mut()
        .flat_map(|f| &mut f.blocks)
        .flat_map(|b| &mut b.operations)
        .find(|op| matches!(op.kind, OperationKind::StoreField { .. }))
        .unwrap();
    if let OperationKind::StoreField { field, .. } = &mut operation.kind {
        *field = 0;
    }
    assert!(
        verify_module(&invalid).is_err(),
        "IR must reject a const field write"
    );
}

#[test]
fn classes_reject_incomplete_construction_const_writes_and_invalid_methods() {
    for text in [
        "class C {const x:Int, y:Int} fn main(){let c=C(x:1);}",
        "class C {const x:Int} fn main(){let c=C(x:1);c.x=2;}",
        "class C {const x:Int} fn main(){let c=C(x:1,x:2);}",
        "class C {const x:Int,x:Int} fn main(){}",
        "class C {x:Int fn x(self:C) {}} fn main(){}",
        "class C {x:Int fn read(self:Int)->Int{return self;}} fn main(){}",
        "class C {x:Int fn read(n:Int,self:C)->Int{return n;}} fn main(){}",
        "class C extends Other {x:Int} fn main(){}",
    ] {
        assert!(compile_source(&source(text)).is_err(), "{text}");
    }
    compile_source(&source(
        "class Box<T> {value:T} fn main(){let b=Box<Int>(value:1);b.value=2;}",
    ))
    .unwrap();
    compile_source(&source("class Inner {value:Int} class Outer {const inner:Inner} fn main(){let o=Outer(inner:Inner(value:1));o.inner.value=2;}" )).unwrap();
}

#[test]
fn const_class_markers_are_independent_nominal_and_recursive() {
    compile_source(&source(include_str!(
        "../../../tests/fixtures/programs/classes/src/main.rinzu"
    )))
    .unwrap();
    compile_source(&source("class C {const value:Float} fn identity(c:C)->C{return c;} async fn main(){let c=C(value:1.5);let copy=await spawn identity(c);}" )).unwrap();
    for text in [
        "class A {const x:Int} class B {const x:Int} fn main(){let same=A(x:1)==B(x:1);}",
        "class Mutable {x:Int} class C {const value:Mutable} fn main(){let c=C(value:Mutable(x:1));let same=c==c;}",
        "class C {const f:fn()->Int} fn f()->Int{return 1;} fn main(){let c=C(f:f);let same=c==c;}",
        "class C {const f:fn()->Int} fn f()->Int{return 1;} fn main(){let m:Map<C,Int> = {};}",
        "class C {const f:fn()->Int} fn f()->Int{return 1;} fn use(c:C){} async fn main(){let c=C(f:f);await spawn use(c);}",
        "class C {const error:Error} fn main(){let c=C(error:error(1,\"x\"));let same=c==c;}",
        "class C {const error:Error} fn main(){let m:Map<C,Int> = {};}",
        "class C {const error:Error} fn identity(c:C)->C{return c;} async fn main(){let c=C(error:error(1,\"x\"));let copy=await spawn identity(c);}",
        "class C {const value:Float} fn main(){let m:Map<C,Int> = {};}",
        "class C {const recursive:C} fn main(){let m:Map<C,Int> = {};}",
    ] {
        assert!(compile_source(&source(text)).is_err(), "{text}");
    }
}
