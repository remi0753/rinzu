use rinzu_runtime::{error_trace::TraceStack, failure::SourcePosition};

fn at(byte: u32) -> Option<SourcePosition> {
    Some(SourcePosition {
        file: 1,
        start_byte: byte,
        end_byte: byte + 1,
    })
}

#[test]
fn disabled_tracing_captures_nothing_and_keeps_no_logical_frames() {
    let mut stack = TraceStack::new(false);
    let token = stack.enter("main", at(1));
    stack.source(at(2));
    assert!(token.is_none());
    assert_eq!(stack.depth(), 0);
    assert!(stack.capture(at(3)).is_none());
    assert!(stack.leave(token));
}

#[test]
fn snapshots_freeze_names_order_and_original_positions_before_frames_return() {
    let mut stack = TraceStack::new(true);
    let outer = stack.enter("main", at(1));
    let mut name = "日本語.leaf".to_owned();
    let inner = stack.enter(&name, at(2));
    name.clear();
    let trace = stack.capture(at(3)).unwrap();
    assert_eq!(trace.frames.len(), 2);
    assert_eq!(&*trace.frames[0].function, "日本語.leaf");
    assert_eq!(trace.frames[0].source, at(3));
    assert_eq!(&*trace.frames[1].function, "main");
    assert_eq!(trace.frames[1].source, at(1));
    assert!(!stack.leave(outer));
    assert!(!stack.leave(Some(usize::MAX)));
    assert_eq!(stack.depth(), 2);
    stack.source(at(99));
    assert!(stack.leave(inner));
    assert!(stack.leave(outer));
    drop(stack);
    assert_eq!(trace.frames[0].source, at(3));
    assert_eq!(trace.frames[1].source, at(1));
}

#[test]
fn tasks_suspend_unrelated_stacks_and_restore_the_synchronous_caller() {
    use rinzu_runtime::error_trace::TraceContext;
    let context = TraceContext::new(true);
    let main = context.enter("main", at(1));
    {
        let _task = context.task("work", at(2));
        let _callee = context.enter("leaf", at(3));
        let trace = context.capture(at(4)).unwrap();
        assert_eq!(
            trace
                .frames
                .iter()
                .map(|frame| frame.function.as_ref())
                .collect::<Vec<_>>(),
            ["leaf", "work"]
        );
    }
    let trace = context.capture(at(5)).unwrap();
    assert_eq!(trace.frames.len(), 1);
    assert_eq!(trace.frames[0].function.as_ref(), "main");
    drop(main);
    assert!(context.capture(None).unwrap().frames.is_empty());
}

#[test]
fn pending_failures_capture_only_normal_errors_and_keep_cleanup_metadata_order() {
    use rinzu_runtime::error_trace::TraceContext;
    use rinzu_runtime::failure::{Diagnostic, ErrorKind, FailureCause, PendingFailure};
    use std::sync::Arc;
    let context = TraceContext::new(true);
    let _main = context.enter("main", at(1));
    let mut pending = PendingFailure::default();
    pending.set_tracer(context.clone());
    pending.fail(Diagnostic {
        cause: FailureCause::Error(ErrorKind::Conversion),
        source: at(2),
    });
    let original = Arc::clone(&pending.current().unwrap().traces[&0]);
    pending.begin_cleanup();
    pending.fail(Diagnostic {
        cause: FailureCause::Error(ErrorKind::Io),
        source: at(3),
    });
    pending.end_cleanup().unwrap();
    let failure = pending.finish().unwrap().unwrap();
    assert!(Arc::ptr_eq(&failure.traces[&0], &original));
    assert_eq!(failure.traces[&1].frames[0].source, at(3));
    assert!(Arc::ptr_eq(
        &failure.diagnostic_at(0).unwrap().traces[&0],
        &original
    ));
    let cleanup = failure.clone().without_primary().unwrap();
    assert!(Arc::ptr_eq(&cleanup.traces[&0], &failure.traces[&1]));
    let mut control = PendingFailure::default();
    control.set_tracer(context);
    control.fail(FailureCause::Cancelled);
    assert!(control.current().unwrap().traces.is_empty());
}

#[test]
fn c_capture_and_cleanup_keep_snapshots_after_logical_stacks_are_released() {
    let directory =
        std::env::temp_dir().join(format!("rinzu-c-trace-execution-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("trace.c"),r#"
#define RINZU_ERROR_TRACE_DEFAULT 1
#include <assert.h>
#include "rinzu/failure.h"
int main(void) {
  rinzu_error_trace_stack_t disabled={.initialized=1};
  rinzu_error_trace_stack_enter(&disabled,"main",4);
  assert(!disabled.frames && !disabled.count);
  assert(!rinzu_error_trace_stack_capture(&disabled,(rinzu_source_position_t){0}));
  rinzu_error_trace_stack_leave(&disabled);
  rinzu_error_trace_stack_t stack={0};
  rinzu_error_trace_stack_enter(&stack,"main",4);
  rinzu_error_trace_stack_source(&stack,(rinzu_source_position_t){1,10,20,1});
  rinzu_error_trace_stack_enter(&stack,"leaf",4);
  rinzu_failure_state_t state={.trace_stack=&stack};
  rinzu_failure_fail(&state,5,(rinzu_source_position_t){1,30,40,1});
  rinzu_error_trace_t *first=state.current.primary_trace;
  assert(first && first->count==2);
  assert(!strcmp(first->frames[0].function,"leaf") && first->frames[0].source.start_byte==30);
  assert(!strcmp(first->frames[1].function,"main") && first->frames[1].source.start_byte==10);
  rinzu_failure_begin(&state);
  rinzu_failure_fail(&state,12,(rinzu_source_position_t){1,50,60,1});
  rinzu_failure_end(&state);
  assert(state.current.primary_trace==first);
  assert(state.current.head->trace!=first && state.current.head->trace->frames[0].source.start_byte==50);
  rinzu_failure_t copied=rinzu_failure_copy(&state.current);
  rinzu_failure_dispose(&state);
  rinzu_error_trace_stack_leave(&stack);
  rinzu_error_trace_stack_leave(&stack);
  assert(!stack.frames && !stack.count && !stack.capacity);
  assert(copied.primary_trace==first && first->frames[0].source.start_byte==30);
  rinzu_failure_release(copied);
  rinzu_failure_t native={.primary=12};
  rinzu_failure_mark_unrecorded_traces(&native);
  assert(native.primary_trace && !native.primary_trace->count);
  rinzu_error_trace_t *original=native.primary_trace;
  rinzu_failure_mark_unrecorded_traces(&native);
  assert(native.primary_trace==original);
  rinzu_failure_release(native);
  rinzu_error_trace_stack_dispose(&stack);
  return 0;
}
"#).unwrap();
    for sanitized in [false, true] {
        let mut compiler = std::process::Command::new("clang");
        compiler.args(["-std=c11", "-O1", "-g", "-Wall", "-Wextra", "-Werror"]);
        if sanitized {
            compiler.arg("-fsanitize=address,undefined");
        }
        let output = compiler
            .arg("-I")
            .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
            .arg(directory.join("trace.c"))
            .arg("-o")
            .arg(directory.join("trace"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let output = std::process::Command::new(directory.join("trace"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    std::fs::remove_dir_all(directory).unwrap();
}
