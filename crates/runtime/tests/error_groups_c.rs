#![cfg(unix)]
use std::process::Command;

#[test]
fn c_error_groups_preserve_order_deduplicate_per_scope_grow_and_reject_control() {
    let root = std::env::temp_dir().join(format!("rinzu-error-groups-c-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("test.c"),r#"
        #include <assert.h>
        #include "rinzu/error_groups.h"
        static rinzu_failure_t error(unsigned position) {
            return (rinzu_failure_t){.primary=15,.primary_source={7,position,position+1,1}};
        }
        int main(int argc,char **argv) {
            rinzu_error_groups_t groups={0};rinzu_error_group_enter(&groups,7);
            if(argc>1) {
                if(argv[1][0]=='s') (void)rinzu_error_group_finish(&groups,8);
                rinzu_failure_t invalid=error(0);
                if(argv[1][0]=='c' && argv[1][1]=='o') invalid.primary=2;
                else {invalid.head=rinzu_failure_allocate(sizeof(*invalid.head));invalid.head->cause=1;invalid.tail=invalid.head;}
                (void)rinzu_error_group_record(&groups,7,0,0,&invalid);
                return 99;
            }
            rinzu_failure_t first=error(10),cleanup=error(11);
            rinzu_error_group_append(&first,cleanup);
            assert(rinzu_error_group_record(&groups,7,1,42,&first));
            rinzu_failure_t duplicate=error(99);
            assert(!rinzu_error_group_record(&groups,7,1,42,&duplicate) && !duplicate.primary);
            rinzu_error_group_enter(&groups,7);
            rinzu_failure_t inner=error(20);assert(rinzu_error_group_record(&groups,7,1,42,&inner));
            inner=rinzu_error_group_aggregate(rinzu_error_group_finish(&groups,7));
            assert(inner.primary_source.start_byte==20 && !inner.head);rinzu_failure_release(inner);
            for(unsigned index=0;index<2;index++) {rinzu_failure_t explicit=error(30);assert(rinzu_error_group_record(&groups,7,0,0,&explicit));}
            rinzu_failure_t combined=rinzu_error_group_aggregate(rinzu_error_group_finish(&groups,7));
            assert(!groups.depth && !groups.top && combined.primary_source.start_byte==10);
            unsigned expected[]={11,30,30},count=0;
            for(rinzu_diagnostic_node_t *node=combined.head;node;node=node->next) {assert(count<3 && node->source.start_byte==expected[count]);count++;}
            assert(count==3);rinzu_failure_release(combined);
            for(int batch=0;batch<3;batch++) {
                rinzu_error_group_enter(&groups,9);
                for(unsigned origin=0;origin<8192;origin++) {rinzu_failure_t failure=error(origin);assert(rinzu_error_group_record(&groups,9,1,origin,&failure));}
                for(unsigned origin=8192;origin-->0;) {rinzu_failure_t failure=error(99);assert(!rinzu_error_group_record(&groups,9,1,origin,&failure));}
                combined=rinzu_error_group_aggregate(rinzu_error_group_finish(&groups,9));
                assert(combined.primary_source.start_byte==0);count=1;
                for(rinzu_diagnostic_node_t *node=combined.head;node;node=node->next) {assert(node->source.start_byte==count);count++;}
                assert(count==8192);rinzu_failure_release(combined);
            }
            rinzu_error_group_enter(&groups,11);first=error(1);rinzu_error_group_record(&groups,11,0,0,&first);
            rinzu_error_group_enter(&groups,12);first=error(2);rinzu_error_group_record(&groups,12,0,0,&first);
            rinzu_error_groups_dispose(&groups);assert(!groups.depth && !groups.top);
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
            .args(["-std=c11", "-Wall", "-Wextra", "-Werror"])
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
        for invalid in ["scope", "control", "cleanup"] {
            let run = Command::new(&binary)
                .arg(invalid)
                .env("ASAN_OPTIONS", "detect_leaks=0")
                .output()
                .unwrap();
            assert_eq!(
                run.status.code(),
                Some(70),
                "{invalid}: {}",
                String::from_utf8_lossy(&run.stderr)
            );
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
