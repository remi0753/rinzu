#![cfg(unix)]
use std::process::Command;

#[test]
fn c_first_error_sources_and_lazy_locations_survive_gc() {
    let root = std::env::temp_dir().join(format!("rinzu-error-sources-c-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/managed_error_sources.c");
    for (name, flags) in [
        ("plain", vec!["-O2"]),
        (
            "san",
            vec![
                "-O1",
                "-g",
                "-fsanitize=address,undefined",
                "-fno-sanitize-recover=all",
            ],
        ),
    ] {
        let binary = root.join(name);
        let compiled = Command::new("clang")
            .args(["-std=c11", "-Wall", "-Wextra", "-Werror", "-pthread"])
            .args(flags)
            .arg("-I")
            .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
            .arg(&fixture)
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let run = Command::new(binary)
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .output()
            .unwrap();
        assert!(
            run.status.success(),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn c_failure_leases_and_list_patterns_preserve_payload_gc_and_domain_boundaries() {
    let root = std::env::temp_dir().join(format!("rinzu-managed-errors-c-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("test.c"),r#"
        #include <assert.h>
        #include "rinzu/managed_errors.h"
        #include "rinzu/error_groups.h"
        #include "rinzu/failure_transport.h"
        static const size_t document_fields[]={1};
        static const size_t error_fields[]={0,0,5,6,2};
        static const size_t cause_fields[]={4};
        static const rinzu_key_variant_t source_variants[]={{0,NULL}};
        static const rinzu_key_variant_t cause_variants[]={{0,NULL},{1,cause_fields}};
        static const rinzu_value_type_t types[]={
            {.kind=RINZU_VALUE_STRING},
            {.kind=RINZU_VALUE_SCALAR,.scalar_kind=RINZU_SCALAR_I64},
            {.kind=RINZU_VALUE_STATE,.identity="Document",.field_count=1,.fields=document_fields},
            {.kind=RINZU_VALUE_ERROR_RECORD,.identity="Failed",.field_count=5,.fields=error_fields},
            {.kind=RINZU_VALUE_ERROR},
            {.kind=RINZU_VALUE_ENUM,.identity="OptionalSource",.variant_count=1,.variants=source_variants},
            {.kind=RINZU_VALUE_ENUM,.identity="OptionalCause",.variant_count=2,.variants=cause_variants},
            {.kind=RINZU_VALUE_LIST,.element=4}
        };
        int main(int argc,char **argv) {
            (void)argv;
            rinzu_value_schema_t schema={0};
            assert(!rinzu_value_schema_init(&schema,types,8));
            assert(!schema.equality[3] && !schema.keys.allowed[3] && !schema.transferable[3]);
            rinzu_managed_data_t data={0};assert(!rinzu_managed_configure_values(&data,&schema));
            rinzu_managed_roots_t roots={0};
            uint64_t document=0,source=0,cause=0,error=0;
            rinzu_managed_slot_t number={7,0};
            assert(!rinzu_managed_composite(&data,2,-1,&number,1,&document));
            assert(!rinzu_managed_composite(&data,5,0,NULL,0,&source));
            assert(!rinzu_managed_composite(&data,6,0,NULL,0,&cause));
            uint64_t name=rinzu_managed_string(&data,(const unsigned char*)"Failed",6);
            uint64_t message=rinzu_managed_string(&data,(const unsigned char*)"message",7);
            rinzu_managed_slot_t fields[]={{name,1},{message,1},{source,1},{cause,1},{document,1}};
            assert(!rinzu_managed_composite(&data,3,-1,fields,5,&error));
            char origin_name[]="origin";
            rinzu_error_trace_frame_t frames[]={
                {origin_name,6,{.present=1,.file=1,.start_byte=17,.end_byte=29}},
                {"main",4,{.present=1,.file=1,.start_byte=3,.end_byte=4}}
            };
            rinzu_error_trace_t *trace=rinzu_error_trace_capture(frames,2);
            origin_name[0]='X';
            assert(!memcmp(trace->frames[0].function,"origin",6));
            assert(!rinzu_managed_error_trace(&data,error));
            assert(rinzu_managed_first_error_trace(&data,error,trace)==trace);
            rinzu_error_trace_t *later=rinzu_error_trace_capture(NULL,0);
            assert(rinzu_managed_first_error_trace(&data,error,later)==trace);
            rinzu_error_trace_release(later);
            rinzu_failure_t failure={.primary=15,.primary_error=rinzu_managed_lease_error(&data,error)};
            if(argc>1) {
                rinzu_managed_data_t other={0};assert(!rinzu_managed_configure_values(&other,&schema));
                (void)rinzu_managed_failure_handle(&other,15,(rinzu_source_position_t){0},failure.primary_error);
                return 99;
            }
            unsigned char packet[34];
            assert(rinzu_failure_transfer_encode(&failure,packet,sizeof packet)==RINZU_TRANSFER_ENCODING);
            rinzu_error_groups_t groups={0};rinzu_error_group_enter(&groups,1);
            assert(rinzu_error_group_record(&groups,1,1,7,&failure));
            assert(!failure.primary);
            rinzu_failure_t duplicate=rinzu_failure_copy(&groups.top->head->failure);
            assert(!rinzu_error_group_record(&groups,1,1,7,&duplicate));
            assert(!duplicate.primary);
            rinzu_managed_collect(&data,&roots);
            assert(rinzu_managed_find(&data,document)->slots[0].bits==7);
            failure=rinzu_error_group_aggregate(rinzu_error_group_finish(&groups,1));
            assert(!groups.depth && !failure.head && failure.primary_error);
            rinzu_failure_t copy=rinzu_failure_copy(&failure);
            rinzu_failure_release(failure);
            (void)rinzu_managed_string(&data,(const unsigned char*)"unrelated local",15);
            for(int i=0;i<3;i++) {
                rinzu_managed_collect(&data,&roots);
                assert(data.objects==6 && rinzu_managed_find(&data,document)->slots[0].bits==7);
                assert(rinzu_managed_error_trace(&data,error)==trace);
                assert(trace->frames[0].source.start_byte==17);
            }
            rinzu_failure_state_t pending={.current=copy};
            assert(rinzu_failure_observe(&pending)==1);
            uint64_t list=rinzu_managed_failure_list(&data,7,&pending.observed->failure);
            rinzu_failure_forget_managed(&pending.observed->failure);
            rinzu_managed_enter(&roots);rinzu_managed_publish(&roots,&list,1);
            const uint64_t specific[]={3},wildcard[]={4},two[]={3,3};
            assert(rinzu_managed_match_errors(&data,list,specific,1,0));
            assert(rinzu_managed_match_errors(&data,list,wildcard,1,1));
            assert(!rinzu_managed_match_errors(&data,list,two,2,0));
            rinzu_managed_collect(&data,&roots);
            assert(!data.error_references && data.objects==7);
            rinzu_managed_find(&data,document)->slots[0].bits=19;
            rinzu_managed_value_t *saved=rinzu_managed_find(&data,error);
            assert(rinzu_managed_find(&data,saved->slots[4].bits)->slots[0].bits==19);
            assert(rinzu_managed_error_metadata(&data,0,error,1)==message);
            rinzu_managed_publish(&roots,NULL,0);rinzu_managed_collect(&data,&roots);
            assert(!data.objects && !data.error_references);
            assert(atomic_load(&trace->references)==1);
            assert(trace->count==2 && !memcmp(trace->frames[0].function,"origin",6));
            rinzu_error_trace_release(trace);
            rinzu_failure_dispose(&pending);rinzu_managed_leave(&roots);
            rinzu_managed_dispose(&roots);rinzu_managed_data_dispose(&data);
            rinzu_value_schema_dispose(&schema);
            return 0;
        }
    "#).unwrap();
    for (name, flags) in [
        ("plain", vec!["-O2"]),
        (
            "san",
            vec![
                "-O1",
                "-g",
                "-fsanitize=address,undefined",
                "-fno-sanitize-recover=all",
            ],
        ),
    ] {
        let binary = root.join(name);
        let compiled = Command::new("clang")
            .args(["-std=c11", "-Wall", "-Wextra", "-Werror", "-pthread"])
            .args(flags)
            .arg("-I")
            .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
            .arg(root.join("test.c"))
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(
            compiled.status.success(),
            "{}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let run = Command::new(&binary)
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .output()
            .unwrap();
        assert!(
            run.status.success(),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
        let invalid = Command::new(&binary)
            .arg("other-domain")
            .env("ASAN_OPTIONS", "detect_leaks=0")
            .output()
            .unwrap();
        assert_eq!(
            invalid.status.code(),
            Some(70),
            "{}",
            String::from_utf8_lossy(&invalid.stderr)
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}
