#![cfg(unix)]
use std::path::PathBuf;
use std::process::Command;

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn packed_source_stores_keep_full_ranges_trace_frames_and_adjacent_fields() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let directory =
        Scratch(std::env::temp_dir().join(format!("rinzu-packed-source-{}", std::process::id())));
    std::fs::create_dir_all(&directory.0).unwrap();
    let helper = std::fs::read_to_string(root.join("crates/driver/src/aot_safepoint.c")).unwrap();
    let setters = [
        "rinzu_aot_source_range_valid",
        "rinzu_baseline_set_source",
        "rinzu_baseline_set_source_packed",
        "rinzu_baseline_observe_source",
    ]
    .map(|name| {
        let name_start = helper.find(&format!("{name}(")).unwrap();
        let start = helper[..name_start].rfind('\n').map_or(0, |i| i + 1);
        let open = name_start + helper[name_start..].find('{').unwrap();
        let mut depth = 0;
        let end = helper[open..]
            .char_indices()
            .find_map(|(offset, ch)| {
                if ch == '{' {
                    depth += 1;
                }
                if ch == '}' {
                    depth -= 1;
                    if depth == 0 {
                        return Some(open + offset + 1);
                    }
                }
                None
            })
            .unwrap();
        &helper[start..end]
    })
    .join("\n");
    assert!(setters.contains("void rinzu_baseline_set_source_packed("));
    let source = directory.0.join("probe.c");
    std::fs::write(
        &source,
        format!(
            r#"
#include <assert.h>
#include <stddef.h>
#include "rinzu/failure.h"
typedef struct {{
  unsigned char prefix[824];
  rinzu_source_position_t source;
  uint64_t canary;
  rinzu_error_trace_stack_t error_trace_stack;
}} RinzuContext;
{setters}
int main(void) {{
  const uint32_t spans[][2]={{{{0,0}},{{0,UINT32_MAX}},{{UINT32_MAX,UINT32_MAX}},
                              {{UINT32_MAX-1,UINT32_MAX}},{{17,29}}}};
  for (unsigned trace=0; trace<2; ++trace) {{
    RinzuContext context={{0}};
    memset(context.prefix,0xa5,sizeof(context.prefix));
    context.canary=UINT64_C(0x0123456789abcdef);
    context.error_trace_stack.initialized=1;
    context.error_trace_stack.enabled=trace;
    rinzu_baseline_set_source_packed(&context,0,0);
    assert(!context.error_trace_stack.count);
    rinzu_error_trace_stack_enter(&context.error_trace_stack,"test",4);
    for (size_t i=0; i<sizeof(spans)/sizeof(*spans); ++i) {{
      uint64_t file=i&1 ? UINT32_MAX : 0;
      uint64_t span=(uint64_t)spans[i][0] | ((uint64_t)spans[i][1]<<32);
      rinzu_baseline_set_source_packed(&context,file,span);
      assert(context.source.file==file && context.source.start_byte==spans[i][0] &&
             context.source.end_byte==spans[i][1] && context.source.present==1);
      if (trace) {{
        rinzu_source_position_t position=context.error_trace_stack.frames[0].source;
        assert(position.file==file && position.start_byte==spans[i][0] &&
               position.end_byte==spans[i][1] && position.present==1);
      }}
      assert(context.canary==UINT64_C(0x0123456789abcdef));
      for (size_t j=0; j<sizeof(context.prefix); ++j) assert(context.prefix[j]==0xa5);
    }}
    rinzu_error_trace_stack_leave(&context.error_trace_stack);
    rinzu_error_trace_stack_dispose(&context.error_trace_stack);
  }}
}}
"#
        ),
    )
    .unwrap();
    for sanitized in [false, true] {
        let binary = directory.0.join("probe");
        let mut build = Command::new("clang");
        build.args(["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror"]);
        if sanitized {
            build.args([
                "-g",
                "-fsanitize=address,undefined",
                "-fno-sanitize-recover=all",
            ]);
        }
        let result = build
            .arg("-I")
            .arg(root.join("include"))
            .arg(&source)
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(result.status.success(), "{result:?}");
        let result = Command::new(&binary)
            .env("ASAN_OPTIONS", "detect_leaks=0:abort_on_error=1")
            .output()
            .unwrap();
        assert!(result.status.success(), "{result:?}");
    }
}
