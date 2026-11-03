use rinzu_runtime::child_scope::{ChildScope, ChildScopeError};
use rinzu_runtime::failure::{Diagnostic, ErrorKind, Failure, FailureCause as C, SourcePosition};
use rinzu_runtime::failure_transport::{failure_descriptor, failure_value};
use rinzu_runtime::transport::{TransportBudget, TransportDirection, encode, preflight};

fn failure(cause: C, file: u64, cleanup: &[C]) -> Failure {
    Failure {
        managed: Default::default(),
        traces: Default::default(),
        primary: Diagnostic {
            cause,
            source: Some(SourcePosition {
                file,
                start_byte: 20,
                end_byte: 30,
            }),
        },
        cleanup: cleanup
            .iter()
            .enumerate()
            .map(|(index, cause)| Diagnostic {
                cause: *cause,
                source: Some(SourcePosition {
                    file,
                    start_byte: index as u32 + 40,
                    end_byte: index as u32 + 41,
                }),
            })
            .collect(),
    }
}

#[test]
fn child_tail_preserves_order_after_handoff_retirement_and_rollback() {
    let directory = std::env::temp_dir().join(format!("rinzu-child-order-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for sanitized in [false, true] {
        let executable = directory.join(if sanitized { "san" } else { "plain" });
        let mut compiler = std::process::Command::new("clang");
        compiler.args(["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread"]);
        if sanitized {
            compiler.args(["-fsanitize=address,undefined", "-fno-sanitize-recover=all"]);
        }
        let output = compiler
            .arg("-I")
            .arg(manifest.join("../../include"))
            .arg(manifest.join("tests/fixtures/child_scope_order.c"))
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let output = std::process::Command::new(executable).output().unwrap();
        assert!(output.status.success(), "{output:?}");
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn aot_child_scope_matches_immutable_rust_observation_and_close_policy() {
    use std::fmt::Write;
    let error = failure(
        C::Error(ErrorKind::Conversion),
        9,
        &[C::Error(ErrorKind::NativeRecordBusy)],
    );
    let trap = failure(C::ArithmeticTrap, 8, &[C::Error(ErrorKind::Conversion)]);
    let parent = failure(C::Cancelled, 1, &[C::Error(ErrorKind::NullNativeValue)]);
    let cancelled = failure(C::Cancelled, 9, &[C::Error(ErrorKind::Conversion)]);
    let cases = [
        (None, vec![(Some(error.clone()), false), (None, false)]),
        (None, vec![(Some(error.clone()), true), (None, false)]),
        (
            Some(parent),
            vec![(Some(cancelled.clone()), false), (None, false)],
        ),
        (
            Some(trap.clone()),
            vec![(Some(error.clone()), false), (Some(trap.clone()), true)],
        ),
        (
            None,
            vec![(Some(trap.clone()), false), (Some(error), false)],
        ),
    ];
    let budget = TransportBudget::new(4096, 4096);
    let encoded = |value: &Failure| {
        let descriptor = failure_descriptor();
        let value = failure_value(value);
        let size = preflight(&descriptor, &value).unwrap();
        encode(
            &descriptor,
            &value,
            budget.reserve(TransportDirection::Result, size).unwrap(),
        )
        .unwrap()
        .bytes()
        .iter()
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join(",")
    };
    let mut program = String::from(
        r#"
#include <assert.h>
#include <string.h>
#include "rinzu/child_scope.h"
#include "rinzu/failure_transport.h"
static void *foreign(void *raw) { rinzu_child_admit(raw, 1); return NULL; }
int main(int argc, char **argv) { (void)argv;
"#,
    );
    let cases = cases
        .into_iter()
        .map(|(parent, children)| (parent, children, None))
        .chain([
            (
                None,
                vec![(Some(cancelled.clone()), false)],
                Some(C::Cancelled),
            ),
            (
                Some(trap.clone()),
                vec![(Some(cancelled.clone()), false)],
                Some(C::Cancelled),
            ),
            (
                None,
                vec![(Some(failure(C::Shutdown, 9, &[])), false)],
                Some(C::Shutdown),
            ),
            (
                Some(trap),
                vec![(Some(cancelled), false)],
                Some(C::Shutdown),
            ),
        ])
        .map(|(parent, children, cancel)| (parent, children, cancel, false))
        .chain([
            (
                None,
                vec![(Some(failure(C::Cancelled, 9, &[])), false)],
                None,
                true,
            ),
            (
                None,
                vec![(
                    Some(failure(C::Cancelled, 9, &[C::Error(ErrorKind::Conversion)])),
                    false,
                )],
                None,
                true,
            ),
            (
                None,
                vec![(Some(failure(C::ArithmeticTrap, 9, &[])), false)],
                None,
                true,
            ),
            (
                Some(failure(C::Shutdown, 1, &[])),
                vec![(
                    Some(failure(C::Shutdown, 9, &[C::Error(ErrorKind::Conversion)])),
                    false,
                )],
                None,
                false,
            ),
        ]);
    for (parent, children, cancel_close, expected_control) in cases {
        let mut scope = ChildScope::new();
        program.push_str("{ rinzu_child_scope_t scope; rinzu_child_init(&scope); rinzu_failure_t parent = {0};\n");
        if let Some(parent) = &parent {
            writeln!(program, "const unsigned char parent_bytes[] = {{{}}}; assert(!rinzu_failure_transfer_decode(parent_bytes, sizeof(parent_bytes), &parent));", encoded(parent)).unwrap();
        }
        for id in 0..children.len() {
            scope.admit(id as u64).unwrap();
            writeln!(program, "assert(rinzu_child_admit(&scope, {id}) == RINZU_CHILD_OK); assert(rinzu_child_admit(&scope, {id}) == RINZU_CHILD_DUPLICATE);").unwrap();
        }
        if expected_control {
            scope.expect_control(0).unwrap();
            program.push_str("assert(!rinzu_child_expect_control(&scope, 0));\n");
        }
        scope.begin_close(parent).unwrap();
        assert_eq!(scope.finish_close(), Err(ChildScopeError::PendingChildren));
        program.push_str("assert(!rinzu_child_begin_close(&scope, &parent)); rinzu_failure_release(parent); assert(rinzu_child_admit(&scope, 99) == RINZU_CHILD_CLOSED); assert(rinzu_child_finish_close(&scope) == RINZU_CHILD_PENDING); assert(rinzu_child_observe(&scope, 0) == RINZU_CHILD_PENDING);\n");
        scope.admit_callback(99).unwrap();
        scope.complete(99, None).unwrap();
        program.push_str("assert(!rinzu_child_admit_with_lease(&scope, 99, 1)); assert(!rinzu_child_complete(&scope, 99, NULL));\n");
        if let Some(cause) = cancel_close {
            let source = Some(SourcePosition {
                file: 7,
                start_byte: 21,
                end_byte: 31,
            });
            if cause == C::Shutdown {
                scope.shutdown_close(source).unwrap();
                program.push_str("assert(!rinzu_child_shutdown_close(&scope, (rinzu_source_position_t){7, 21, 31, 1}));\n");
                scope.cancel_close(None).unwrap();
                program.push_str(
                    "assert(!rinzu_child_cancel_close(&scope, (rinzu_source_position_t){0}));\n",
                );
            } else {
                scope.cancel_close(source).unwrap();
                program.push_str("assert(!rinzu_child_cancel_close(&scope, (rinzu_source_position_t){7, 21, 31, 1}));\n");
                scope.shutdown_close(None).unwrap();
                program.push_str(
                    "assert(!rinzu_child_shutdown_close(&scope, (rinzu_source_position_t){0}));\n",
                );
            }
        }
        // Deliver in reverse order; report order follows creation identities.
        for (id, (failure, observed)) in children.into_iter().enumerate().rev() {
            scope.complete(id as u64, failure.clone()).unwrap();
            writeln!(program, "{{ rinzu_failure_t failure = {{0}};").unwrap();
            if let Some(failure) = &failure {
                writeln!(program, "const unsigned char bytes[] = {{{}}}; assert(!rinzu_failure_transfer_decode(bytes, sizeof(bytes), &failure));", encoded(failure)).unwrap();
            }
            writeln!(program, "assert(!rinzu_child_complete(&scope, {id}, &failure)); assert(rinzu_child_complete(&scope, {id}, &failure) == RINZU_CHILD_COMPLETE); rinzu_failure_release(failure); }}").unwrap();
            if observed {
                scope.observe(id as u64).unwrap();
                scope.observe(id as u64).unwrap();
                writeln!(program, "assert(!rinzu_child_observe(&scope, {id})); assert(!rinzu_child_observe(&scope, {id}));").unwrap();
            }
        }
        let outcome = scope.finish_close().unwrap();
        if expected_control && let Some(failure) = &outcome {
            assert!(matches!(
                failure.primary.cause,
                C::ArithmeticTrap | C::Error(ErrorKind::UnobservedChildFailure)
            ));
            assert!(failure.cleanup.iter().all(|d| d.cause != C::Cancelled));
            if failure.primary.cause != C::ArithmeticTrap {
                assert!(!failure.cleanup.is_empty());
            }
        }
        program.push_str("assert(!rinzu_child_finish_close(&scope)); assert(!scope.children && scope.finished); assert(!rinzu_child_finish_close(&scope));\n");
        assert_eq!(scope.admit_callback(100), Err(ChildScopeError::Closed));
        program.push_str(
            "assert(rinzu_child_admit_with_lease(&scope, 100, 1) == RINZU_CHILD_CLOSED);\n",
        );
        assert_eq!(scope.cancel_close(None), Err(ChildScopeError::Closed));
        assert_eq!(scope.shutdown_close(None), Err(ChildScopeError::Closed));
        program.push_str("assert(rinzu_child_cancel_close(&scope, (rinzu_source_position_t){0}) == RINZU_CHILD_CLOSED);\n");
        program.push_str("assert(rinzu_child_shutdown_close(&scope, (rinzu_source_position_t){0}) == RINZU_CHILD_CLOSED);\n");
        if let Some(outcome) = &outcome {
            writeln!(program, "const unsigned char expected[] = {{{}}}; unsigned char actual[sizeof(expected)]; assert(!rinzu_failure_transfer_encode(&scope.outcome, actual, sizeof(actual))); assert(!memcmp(actual, expected, sizeof(actual)));", encoded(outcome)).unwrap();
        } else {
            program.push_str("assert(!scope.outcome.primary && !scope.outcome.head);\n");
        }
        program.push_str("rinzu_child_dispose(&scope); }\n");
    }
    program.push_str(r#"
    {rinzu_child_scope_t scope;rinzu_child_init(&scope);
    assert(!rinzu_child_admit(&scope,0));assert(!rinzu_child_complete(&scope,0,NULL));assert(rinzu_child_retire_unreferenced(&scope,0));
    assert(!rinzu_child_admit(&scope,1));rinzu_failure_t error={.primary=8};assert(!rinzu_child_complete(&scope,1,&error));
    assert(!rinzu_child_retire_unreferenced(&scope,1));assert(!rinzu_child_observe(&scope,1));assert(rinzu_child_retire_unreferenced(&scope,1));
    assert(!rinzu_child_admit(&scope,2));rinzu_failure_t cancelled={.primary=2};assert(!rinzu_child_complete(&scope,2,&cancelled));
    assert(!rinzu_child_retire_unreferenced(&scope,2));assert(!rinzu_child_expect_control(&scope,2));assert(rinzu_child_retire_unreferenced(&scope,2));
    assert(!rinzu_child_admit(&scope,3));rinzu_diagnostic_node_t cleanup={.cause=8};cancelled.head=cancelled.tail=&cleanup;
    assert(!rinzu_child_complete(&scope,3,&cancelled));assert(!rinzu_child_expect_control(&scope,3));assert(!rinzu_child_retire_unreferenced(&scope,3));
    assert(!rinzu_child_observe(&scope,3));assert(rinzu_child_retire_unreferenced(&scope,3));
    assert(!rinzu_child_begin_close(&scope,NULL));assert(!rinzu_child_finish_close(&scope));rinzu_child_dispose(&scope);}
    {rinzu_child_scope_t source, supervisor;
    rinzu_child_init(&source);rinzu_child_init(&supervisor);
    assert(!rinzu_child_admit(&source,1));assert(!rinzu_child_admit(&source,2));
    rinzu_failure_t failed={.primary=12};
    assert(!rinzu_child_complete(&source,2,&failed));assert(!rinzu_child_observe(&source,2));
    assert(!rinzu_child_admit(&source,3));assert(!rinzu_child_expect_control(&source,3));
    assert(!rinzu_child_begin_close(&supervisor,NULL));
    assert(!rinzu_child_handoff(&source,&supervisor));assert(!source.children);
    assert(!rinzu_child_begin_close(&source,NULL));assert(!rinzu_child_finish_close(&source));
    rinzu_failure_t cancelled={.primary=2};
    assert(!rinzu_child_complete(&supervisor,1,NULL));assert(!rinzu_child_complete(&supervisor,3,&cancelled));
    assert(!rinzu_child_finish_close(&supervisor));assert(!supervisor.outcome.primary);
    rinzu_child_dispose(&source);rinzu_child_dispose(&supervisor);}
    "#);
    program.push_str("{rinzu_child_scope_t scope; rinzu_child_init(&scope); assert(!rinzu_child_admit(&scope,1)); assert(!rinzu_child_withdraw_admission(&scope,1)); assert(rinzu_child_complete(&scope,1,NULL)==RINZU_CHILD_MISSING); assert(!rinzu_child_admit(&scope,2)); assert(!rinzu_child_complete(&scope,2,NULL)); assert(rinzu_child_withdraw_admission(&scope,2)==RINZU_CHILD_COMPLETE); assert(!rinzu_child_begin_close(&scope,NULL)); assert(!rinzu_child_finish_close(&scope)); rinzu_child_dispose(&scope);}\n");
    program.push_str("rinzu_child_scope_t scope; rinzu_child_init(&scope); if (argc > 1) { pthread_t worker; assert(!pthread_create(&worker, NULL, foreign, &scope)); assert(!pthread_join(worker, NULL)); } assert(!rinzu_child_begin_close(&scope, NULL)); assert(!rinzu_child_finish_close(&scope)); rinzu_child_dispose(&scope); return 0; }\n");
    let directory = std::env::temp_dir().join(format!("rinzu-child-scope-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("test.c"), program).unwrap();
    let output = std::process::Command::new("clang")
        .args([
            "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread", "-I",
        ])
        .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
        .arg(directory.join("test.c"))
        .arg("-o")
        .arg(directory.join("test"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = std::process::Command::new(directory.join("test"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let foreign = std::process::Command::new(directory.join("test"))
        .arg("foreign")
        .output()
        .unwrap();
    assert_eq!(foreign.status.code(), Some(70));
    assert_eq!(budget.snapshot().used_bytes, 0);
    std::fs::remove_dir_all(directory).unwrap();
}
