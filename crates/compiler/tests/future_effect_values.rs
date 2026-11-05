use rinzu_compiler::{DiagnosticCode, FileId, SourceFile, compile_source};

const FAILURES: &str = r#"
import rinzu.list;
import rinzu.map;
error A{} error B{}
async fn a(){throw A(message:"a");}
async fn b(){throw B(message:"b");}
"#;
fn accepted(text: &str) {
    compile_source(&SourceFile::new(
        FileId(241),
        "future-values.rinzu",
        format!("{FAILURES}{text}"),
    ))
    .unwrap();
}
fn incomplete(text: &str) {
    let diagnostics = compile_source(&SourceFile::new(
        FileId(241),
        "future-values.rinzu",
        format!("{FAILURES}{text}"),
    ))
    .unwrap_err();
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == DiagnosticCode::NonExhaustiveCatch),
        "{diagnostics:#?}"
    );
}

#[test]
fn synchronous_and_generic_factories_preserve_the_actual_future_failure() {
    for make in ["identity(a())", "generic<Future<Unit>>(identity(a()))"] {
        accepted(&format!(
            r#"
fn identity(value:Future<Unit>)->Future<Unit>{{return value;}}
fn generic<T>(value:T)->T{{return value;}}
async fn main(){{try{{await {make};}}catch[A]{{}}}}
"#
        ));
        incomplete(&format!(
            r#"
fn identity(value:Future<Unit>)->Future<Unit>{{return value;}}
fn generic<T>(value:T)->T{{return value;}}
async fn main(){{try{{await {make};}}catch[B]{{}}}}
"#
        ));
    }
    accepted(
        r#"
async fn several(){try{throw A(message:"a");}finally{throw B(message:"b");}}
fn identity(value:Future<Unit>)->Future<Unit>{return value;}
async fn main(){try{await identity(several());}catch[A,B]{}}
"#,
    );
    incomplete(
        r#"
async fn several(){try{throw A(message:"a");}finally{throw B(message:"b");}}
fn identity(value:Future<Unit>)->Future<Unit>{return value;}
async fn main(){try{await identity(several());}catch[A]{}}
"#,
    );
}

#[test]
fn named_constructor_fields_preserve_future_provenance_with_reordered_fields() {
    accepted(
        r#"
class Pending {const left:Future<Unit>,const right:Future<Unit>}
fn pack(first:Future<Unit>,second:Future<Unit>)->Pending{return Pending(right:second,left:first);}
fn select(job:Pending)->Future<Unit>{return job.left;}
async fn main(){let first=a();let second=b();try{await select(pack(first,second));}catch[A]{}try{await second;}catch[B]{}}
"#,
    );
}

#[test]
fn recursive_parameter_projections_widen_without_losing_possible_failures() {
    accepted(
        r#"
class Node {const future:Future<Unit>,const next:Option<Node>}
fn pick(node:Node,count:Int)->Future<Unit>{
if count==0{return node.future;}
switch node.next {
case Option.None:{return node.future;}
case Option.Some(next):{return pick(next,count-1);}
}}
async fn main(){let first=a();let second=b();let tail=Node(future:second,next:Option<Node>.None());let root=Node(future:first,next:Option<Node>.Some(value:tail));try{await pick(root,1);}catch errors{}try{await first;}catch[A]{}try{await second;}catch[B]{}}
"#,
    );
    incomplete(
        r#"
class Node {const future:Future<Unit>,const next:Option<Node>}
fn pick(node:Node,count:Int)->Future<Unit>{
if count==0{return node.future;}
switch node.next {
case Option.None:{return node.future;}
case Option.Some(next):{return pick(next,count-1);}
}}
async fn main(){let first=a();let second=b();let tail=Node(future:second,next:Option<Node>.None());let root=Node(future:first,next:Option<Node>.Some(value:tail));try{await pick(root,1);}catch[A]{}try{await first;}catch[A]{}try{await second;}catch[B]{}}
"#,
    );
}

#[test]
fn immutable_record_and_const_class_fields_preserve_nested_future_values() {
    for declaration in [
        "type Pending={future:Future<Unit>};fn pack(value:Future<Unit>)->Pending{return {future=value};}",
        "class Pending {const future:Future<Unit>} fn pack(value:Future<Unit>)->Pending{return Pending(future:value);}",
    ] {
        accepted(&format!(
            r#"
{declaration}
fn unpack(value:Pending)->Future<Unit>{{return value.future;}}
async fn main(){{let job=pack(a());try{{await unpack(job);}}catch[A]{{}}}}
"#
        ));
        incomplete(&format!(
            r#"
{declaration}
fn unpack(value:Pending)->Future<Unit>{{return value.future;}}
async fn main(){{let job=pack(b());try{{await unpack(job);}}catch[A]{{}}}}
"#
        ));
    }
    accepted(
        r#"async fn main(){let job={inner={future=a()}};try{await job.inner.future;}catch[A]{}}"#,
    );
}

#[test]
fn enum_fields_and_switch_bindings_keep_their_future_effect() {
    accepted(
        r#"
enum Pending {Job(future:Future<Unit>), None}
fn pack(value:Future<Unit>)->Pending{return Pending.Job(future:value);}
async fn main(){switch pack(a()) {
case Pending.Job(future):{try{await future;}catch[A]{}}
case Pending.None:{}
}}
"#,
    );
    incomplete(
        r#"
enum Pending {Job(future:Future<Unit>), None}
fn pack(value:Future<Unit>)->Pending{return Pending.Job(future:value);}
async fn main(){switch pack(b()) {
case Pending.Job(future):{try{await future;}catch[A]{}}
case Pending.None:{}
}}
"#,
    );
}

#[test]
fn persistent_list_and_map_operations_and_factories_keep_latent_errors() {
    for (helper, call) in [
        (
            "fn first(values:List<Future<Unit>>)->Option<Future<Unit>>{return rinzu.list.get(values,0);}",
            "first(rinzu.list.append([pending],pending))",
        ),
        (
            "fn first(values:Map<String,Future<Unit>>)->Option<Future<Unit>>{return rinzu.map.get(values,\"key\");}",
            "first(rinzu.map.insert({\"key\":pending},\"other\",pending))",
        ),
    ] {
        accepted(&format!(
            r#"
{helper}
async fn main(){{let pending=a();switch {call} {{
case Option.Some(future):{{try{{await future;}}catch[A]{{}}}}
case Option.None:{{}}
}}try{{await pending;}}catch[A]{{}}}}
"#
        ));
        incomplete(&format!(
            r#"
{helper}
async fn main(){{let pending=a();switch {call} {{
case Option.Some(future):{{try{{await future;}}catch[B]{{}}}}
case Option.None:{{}}
}}try{{await pending;}}catch[A]{{}}}}
"#
        ));
    }
}

#[test]
fn mutable_class_aliases_are_not_treated_as_immutable_future_facts() {
    incomplete(
        r#"
class Pending {future:Future<Unit>}
async fn main(){let original=a();let replacement=b();let job=Pending(future:original);let alias=job;alias.future=replacement;try{await job.future;}catch[A]{}try{await original;}catch[A]{}try{await replacement;}catch[B]{}}
"#,
    );
    accepted(
        r#"
class Pending {future:Future<Unit>}
async fn main(){let original=a();let replacement=b();let job=Pending(future:original);let alias=job;alias.future=replacement;try{await job.future;}catch errors{}try{await original;}catch[A]{}try{await replacement;}catch[B]{}}
"#,
    );
}

#[test]
fn branches_keep_the_union_and_recursive_immutable_shapes_reach_a_fixed_point() {
    accepted(
        r#"
fn choose(flag:Bool,left:Future<Unit>,right:Future<Unit>)->Future<Unit>{if flag{return left;}return right;}
async fn main(){let first=a();let second=b();try{await choose(true,first,second);}catch[A]{}catch[B]{}try{await first;}catch[A]{}try{await second;}catch[B]{}}
"#,
    );
    incomplete(
        r#"
fn choose(flag:Bool,left:Future<Unit>,right:Future<Unit>)->Future<Unit>{if flag{return left;}return right;}
async fn main(){let first=a();let second=b();try{await choose(true,first,second);}catch[A]{}try{await first;}catch[A]{}try{await second;}catch[B]{}}
"#,
    );
    accepted(
        r#"
class Pending {const future:Future<Unit>, const next:Option<Pending>}
fn nested(count:Int,value:Future<Unit>)->Pending{
if count==0{return Pending(future:value,next:Option<Pending>.None());}
return Pending(future:value,next:Option<Pending>.Some(value:nested(count-1,value)));
}
async fn main(){let job=nested(3,a());try{await job.future;}catch[A]{}}
"#,
    );
}
