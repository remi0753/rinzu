use rinzu_runtime::child_scope::ChildScope;
use rinzu_runtime::failure::{ErrorKind, Failure, FailureCause};

#[test]
fn all_ledger_deduplicates_origins_and_keeps_independent_statement_failures() {
    let error = Failure::new(FailureCause::Error(ErrorKind::Conversion));
    let mut scope = ChildScope::all();
    scope.admit(1).unwrap();
    scope.admit(2).unwrap();
    scope.record_error(Some(1), error.clone()).unwrap();
    scope.record_error(Some(1), error.clone()).unwrap();
    scope.record_error(None, error.clone()).unwrap();
    scope.record_error(None, error.clone()).unwrap();
    assert!(!scope.cancels_on_failure(&error));
    assert!(scope.cancels_on_failure(&Failure::new(FailureCause::Cancelled)));
    scope.begin_close(None).unwrap();
    assert_eq!(scope.pending().unwrap(), vec![1, 2]);
    scope.complete(1, Some(error.clone())).unwrap();
    scope.complete(2, Some(error)).unwrap();
    let failure = scope.finish_close().unwrap().unwrap();
    assert_eq!(scope.errors().len(), 4);
    assert_eq!(failure.cleanup.len(), 3);
    assert_eq!(scope.finish_close().unwrap(), Some(failure));

    let directory = std::env::temp_dir().join(format!("rinzu-all-ledger-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let source = r#"
#include <assert.h>
#include "rinzu/child_scope.h"
int main(void) {
  rinzu_child_scope_t scope; rinzu_child_init_all(&scope);
  rinzu_failure_t error={.primary=5}, cancelled={.primary=2};
  assert(!rinzu_child_admit(&scope,1)); assert(!rinzu_child_admit(&scope,2));
  assert(!rinzu_child_record_error(&scope,1,1,&error));
  assert(!rinzu_child_record_error(&scope,1,1,&error));
  assert(!rinzu_child_record_error(&scope,0,0,&error));
  assert(!rinzu_child_record_error(&scope,0,0,&error));
  assert(!rinzu_child_cancels_on_failure(&scope,&error));
  assert(rinzu_child_cancels_on_failure(&scope,&cancelled));
  assert(!rinzu_child_begin_close(&scope,NULL));
  assert(rinzu_child_finish_close(&scope)==RINZU_CHILD_PENDING);
  assert(!rinzu_child_complete(&scope,1,&error)); assert(!rinzu_child_complete(&scope,2,&error));
  assert(!rinzu_child_finish_close(&scope));
  unsigned errors=0, cleanup=0;
  for(rinzu_child_error_t *e=scope.errors;e;e=e->next) ++errors;
  for(rinzu_diagnostic_node_t *n=scope.outcome.head;n;n=n->next) ++cleanup;
  assert(errors==4 && cleanup==3 && scope.outcome.primary==5);
  assert(!rinzu_child_finish_close(&scope));
  rinzu_child_dispose(&scope);
  rinzu_child_init_all(&scope);
  rinzu_failure_t shutdown={.primary=4};
  assert(!rinzu_child_admit(&scope,1));
  assert(!rinzu_child_begin_close(&scope,&shutdown));
  assert(!rinzu_child_complete(&scope,1,&shutdown));
  assert(!rinzu_child_finish_close(&scope));
  assert(scope.outcome.primary==4 && !scope.outcome.head);
  rinzu_child_dispose(&scope);
  return 0;
}
"#;
    std::fs::write(directory.join("all.c"), source).unwrap();
    let executable = directory.join("all");
    let build = std::process::Command::new("clang")
        .args([
            "-std=c11",
            "-Werror",
            "-Wall",
            "-Wextra",
            "-pthread",
            "-fsanitize=address,undefined",
            "-I",
        ])
        .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
        .arg(directory.join("all.c"))
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(build.status.success(), "{build:?}");
    let run = std::process::Command::new(executable).output().unwrap();
    assert!(run.status.success(), "{run:?}");
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn all_group_shutdown_does_not_duplicate_child_control_but_keeps_cleanup_error() {
    for cleanup in [false, true] {
        let mut scope = ChildScope::all();
        scope.admit(1).unwrap();
        scope
            .begin_close(Some(Failure::new(FailureCause::Shutdown)))
            .unwrap();
        let mut child = Failure::new(FailureCause::Shutdown);
        if cleanup {
            child
                .cleanup
                .push(FailureCause::Error(ErrorKind::Conversion).into());
        }
        scope.complete(1, Some(child)).unwrap();
        let outcome = scope.finish_close().unwrap().unwrap();
        assert_eq!(outcome.primary.cause, FailureCause::Shutdown);
        assert_eq!(outcome.cleanup.len(), usize::from(cleanup));
        assert!(
            outcome
                .cleanup
                .iter()
                .all(|e| e.cause != FailureCause::Shutdown)
        );
    }
}
