#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use rinzu_codegen::{
    BaselineOptions, SourceAsyncOptions, run_baseline_jit_with_options, run_source_async_report,
};
use rinzu_compiler::{FileId, SourceFile, compile_source};

#[test]
fn native_record_references_and_field_display_survive_gc_and_captures() {
    let text = r#"
        import rinzu.stdio;
        native {record Point{x:Int,valid:Bool}}
        class Holder{const point:Point}
        type Context={point:Point};
        fn hold(value:Point)->Point{return value;}
        fn main(){
            try{
            let point=Point(x:7,valid:true);
            let context:Context={point=hold(point)};
            let holder=Holder(point:context.point);
            let read=fn()->Point{return holder.point;};
            let selected=read();
            var index=0;
            while index<40{
                let transient=Point(x:index,valid:false);
                index=index+1;
            }
            rinzu.stdio.writeLine(selected);
            rinzu.stdio.writeLine(context.point);
            }catch errors{panic("unexpected native record failure");}
        }
    "#;
    let artifact =
        compile_source(&SourceFile::new(FileId(1), "native-shared.rinzu", text)).unwrap();
    let run = run_baseline_jit_with_options(
        &artifact.ir,
        "main",
        BaselineOptions {
            capture_stdout: true,
            collect_every_allocation: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(run.result, Ok(0), "{run:?}");
    assert_eq!(
        run.stdout,
        b"Point(x: 7, valid: true)\nPoint(x: 7, valid: true)\n"
    );
    assert_eq!(run.heap_objects, 0);
    assert!(run.collection_count > 40);

    let text = text.replace("fn main()", "async fn main()");
    let artifact = compile_source(&SourceFile::new(
        FileId(2),
        "native-shared-async.rinzu",
        text,
    ))
    .unwrap();
    let run = run_source_async_report(
        &artifact.ir,
        "main",
        SourceAsyncOptions {
            capture_stdout: true,
            gc_stress: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(run.failure_details.is_none(), "{run:?}");
    assert_eq!(
        run.stdout,
        b"Point(x: 7, valid: true)\nPoint(x: 7, valid: true)\n"
    );
    assert_eq!((run.heap.objects, run.heap.roots), (0, 0));
    assert!(run.heap.collections > 40);
}
