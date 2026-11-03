use std::process::Command;

#[test]
fn future_index_survives_growth_collisions_retirement_and_repeated_batches() {
    let directory = std::env::temp_dir().join(format!("rinzu-future-index-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for sanitized in [false, true] {
        let executable = directory.join(if sanitized { "san" } else { "plain" });
        let mut compiler = Command::new("clang");
        compiler.args(["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread"]);
        if sanitized {
            compiler.args(["-fsanitize=address,undefined", "-fno-sanitize-recover=all"]);
        }
        let output = compiler
            .arg("-I")
            .arg(manifest.join("../../include"))
            .arg(manifest.join("tests/fixtures/future_index.c"))
            .arg("-o")
            .arg(&executable)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let output = Command::new(&executable).output().unwrap();
        assert!(output.status.success(), "{output:?}");
        for mode in ["missing", "overflow", "future-oom", "child-oom"] {
            let output = Command::new(&executable).arg(mode).output().unwrap();
            assert_eq!(
                output.status.code(),
                Some(if mode.contains("oom") { 71 } else { 70 }),
                "{mode}: {output:?}"
            );
            if mode.contains("oom") {
                assert!(
                    String::from_utf8_lossy(&output.stderr).contains("resource unavailable (71)")
                );
            } else {
                assert!(output.stderr.is_empty(), "{mode}: {output:?}");
            }
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn aot_worker_materializes_on_receivers_and_settles_exact_transport() {
    let directory = std::env::temp_dir().join(format!(
        "rinzu-aot-worker-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let source = directory.join("worker.c");
    let executable = directory.join("worker");
    std::fs::write(
        &source,
        r#"
#include <assert.h>
#include "rinzu/source_async.h"
static const unsigned kinds[] = {RINZU_SCALAR_I8, RINZU_SCALAR_U64, RINZU_SCALAR_F32, RINZU_SCALAR_BOOL};
static const uint64_t values[] = {UINT64_MAX, UINT64_MAX, UINT64_C(0x80000000), 1};
static void mixed(rinzu_async_future_t *future) {
  assert(!pthread_equal(future->runtime->owner, pthread_self()));
  assert(future->runtime->transport_bytes == 0 && future->runtime->domains == 1);
  assert(future->parameter_count == 4);
  for (size_t i = 0; i < 4; ++i) assert(future->arguments[i] == values[i]);
  future->result_slot = UINT64_C(0x7fc01234);
}
static void failed(rinzu_async_future_t *future) {
  assert(!future->input && future->input_size == 0);
  future->failure.primary = 1;
  future->failure.primary_source = (rinzu_source_position_t){9, 17, 23, 1};
  rinzu_diagnostic_node_t *first = rinzu_failure_allocate(sizeof(*first));
  first->cause = 5; first->source = (rinzu_source_position_t){8, 20, 22, 1};
  rinzu_diagnostic_node_t *second = rinzu_failure_allocate(sizeof(*second)); second->cause = 8;
  first->next = second; future->failure.head = first; future->failure.tail = second;
  rinzu_observed_error_t *observed = rinzu_failure_allocate(sizeof(*observed));
  observed->failure.primary = 5; observed->failure.primary_source = (rinzu_source_position_t){7, 30, 40, 1};
  future->observed = observed;
}
static void empty(rinzu_async_future_t *future) {
  assert(future->parameter_count == 0 && !future->arguments);
  future->result_slot = 0;
}
int main(int argc, char **argv) {
  (void)argv;
  rinzu_async_runtime_t runtime; rinzu_async_init(&runtime);
  rinzu_source_position_t source = {1, 2, 3, 1};
  rinzu_async_future_t *future = rinzu_async_worker(&runtime, kinds, values, 4, RINZU_SCALAR_F32, mixed, source);
  assert(rinzu_async_handle(future) == RINZU_ASYNC_HANDLE_BASE + 1);
  assert(rinzu_async_find(&runtime, rinzu_async_handle(future)) == future);
  assert(future->input_size == 14 && runtime.transport_bytes == 0);
  assert(rinzu_async_admit(future));
  assert(runtime.transport_bytes == 14 && runtime.in_flight == 1);
  assert(future->input[0] == 255 && future->input[13] == 1);
  rinzu_async_start(future); pthread_t worker = future->thread;
  rinzu_async_join(future);
  assert(runtime.transport_bytes == 4 && runtime.domains == 1);
  assert(future->result_slot == 0 && future->result_size == 4);
  assert(future->result[0] == 0x34 && future->result[1] == 0x12 && future->result[2] == 0xc0 && future->result[3] == 0x7f);
  assert(rinzu_async_materialize(future) == UINT64_C(0x7fc01234));
  assert(rinzu_async_materialize(future) == UINT64_C(0x7fc01234));
  assert(!runtime.transport_bytes && !runtime.domains && !runtime.in_flight);
  assert(runtime.peak_transport_bytes == 14 && runtime.peak_domains == 1 && runtime.peak_in_flight == 1);
  rinzu_async_release(future);
  future = rinzu_async_worker(&runtime, kinds, values, 4, RINZU_SCALAR_U64, failed, source);
  rinzu_async_execute(future);
  assert(!future->failure.primary && !future->failure.head && !future->observed);
  assert(future->result_is_failure && future->result_size == 70 && future->observed_size == 34);
  assert(runtime.transport_bytes == 104 && runtime.domains == 1);
  rinzu_async_materialize(future);
  assert(future->failure.primary == 1 && future->failure.primary_source.file == 9);
  assert(future->failure.head->cause == 5 && future->failure.head->source.file == 8 && future->failure.tail->cause == 8);
  assert(!runtime.transport_bytes && !runtime.domains && !runtime.in_flight);
  rinzu_failure_state_t observed = {0}; rinzu_async_adopt_observed(future, &observed);
  assert(observed.observed_count == 1 && observed.observed->failure.primary_source.file == 7);
  rinzu_async_materialize(future); rinzu_async_adopt_observed(future, &observed);
  assert(observed.observed_count == 1 && future->failure.primary == 1);
  rinzu_failure_dispose(&observed); rinzu_async_release(future);
  /* Reject a partial diagnostic response without making capacity rejection
     process fatal. The synthetic occupied credit requires no large buffer. */
  runtime.transport_bytes = 256 * 1024 * 1024 - 80;
  size_t occupied = runtime.transport_bytes;
  future = rinzu_async_worker(&runtime, kinds, values, 4, RINZU_SCALAR_U64, failed, source);
  rinzu_async_execute(future);
  assert(future->failure.primary == 3 && future->failure.primary_source.file == 1);
  assert(!future->result && !future->observed_packets && !future->observed);
  assert(runtime.transport_bytes == occupied && runtime.domains == 1);
  rinzu_async_materialize(future); rinzu_async_release(future);
  assert(runtime.transport_bytes == occupied && !runtime.domains && !runtime.in_flight);
  runtime.transport_bytes = 0;
  future = rinzu_async_worker(&runtime, NULL, NULL, 0, RINZU_TRANSFER_UNIT, empty, source);
  assert(rinzu_async_admit(future));
  rinzu_async_execute(future); assert(rinzu_async_materialize(future) == 0); rinzu_async_release(future);
  uint64_t ready = rinzu_async_ready(&runtime, RINZU_SCALAR_U64, UINT64_MAX, source);
  future = rinzu_async_find(&runtime, ready);
  assert(ready < (UINT64_C(1) << 62));
  assert(rinzu_async_materialize(future) == UINT64_MAX); rinzu_async_release(future);
  /* Admission stays live until the receiver finishes, including cancellation
     before thread creation and exhaustion with an already encoded input. */
  for (unsigned i = 0; i < 8; ++i) {
    future = rinzu_async_worker(&runtime, kinds, values, 4, RINZU_SCALAR_F32, mixed, source);
    assert(rinzu_async_admit(future));
    future->failure.primary = 2;
  }
  future = rinzu_async_worker(&runtime, kinds, values, 4, RINZU_SCALAR_F32, mixed, source);
  assert(!rinzu_async_admit(future));
  assert(runtime.transport_bytes == 112 && runtime.domains == 8 && runtime.in_flight == 8);
  rinzu_async_release(future);
  /* A deferred request has no encoded buffer or admission to settle. */
  rinzu_async_worker(&runtime, kinds, values, 4, RINZU_SCALAR_F32, mixed, source);
  if (argc > 1) { runtime.owner = worker; rinzu_async_find(&runtime, ready); }
  rinzu_async_dispose(&runtime);
  assert(!runtime.records && !runtime.transport_bytes && !runtime.domains && !runtime.in_flight);
  return 0;
}
"#,
    )
    .unwrap();
    let output = Command::new("clang")
        .args(["-std=c11", "-Wall", "-Wextra", "-Werror", "-pthread"])
        .arg("-I")
        .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../include"))
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(Command::new(&executable).status().unwrap().success());
    let foreign = Command::new(&executable)
        .arg("foreign-owner")
        .output()
        .unwrap();
    assert_eq!(foreign.status.code(), Some(70));
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn aot_child_start_preserves_prefix_and_cancel_waits_for_physical_return_and_owner_decode() {
    let directory = std::env::temp_dir().join(format!("rinzu-aot-eager-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("test.c"), r#"
#include <assert.h>
#include "rinzu/source_async.h"
typedef struct {
  pthread_mutex_t mutex;
  pthread_cond_t condition;
  int entered, released, failed;
  pthread_t thread;
} control_t;
static void slow(rinzu_async_future_t *future) {
  control_t *control = (void *)future->native_payload;
  assert(!pthread_equal(future->runtime->owner, pthread_self()));
  assert(!pthread_mutex_lock(&control->mutex));
  control->entered = 1; control->thread = pthread_self();
  assert(!pthread_cond_broadcast(&control->condition));
  while (!control->released) assert(!pthread_cond_wait(&control->condition, &control->mutex));
  assert(!pthread_mutex_unlock(&control->mutex));
  if (control->failed) {
    future->failure.primary = 1; future->failure.primary_source = (rinzu_source_position_t){9, 11, 13, 1};
    rinzu_child_append(&future->failure, 5, (rinzu_source_position_t){10, 21, 23, 1});
  } else future->result_slot = UINT64_MAX;
}
static void fast(rinzu_async_future_t *future) { future->result_slot = 0; }
static void never(rinzu_async_future_t *future) { (void)future; assert(!"cancelled body must not run"); }
static control_t *attach(rinzu_async_future_t *future, int failed) {
  control_t *control = rinzu_failure_allocate(sizeof(*control));
  assert(!pthread_mutex_init(&control->mutex, NULL));
  assert(!pthread_cond_init(&control->condition, NULL));
  control->failed = failed; future->native_payload = (void *)control;
  return control;
}
static void entered(control_t *control) {
  assert(!pthread_mutex_lock(&control->mutex));
  while (!control->entered) assert(!pthread_cond_wait(&control->condition, &control->mutex));
  assert(!pthread_equal(control->thread, pthread_self()));
  assert(!pthread_mutex_unlock(&control->mutex));
}
static void release(control_t *control) {
  assert(!pthread_mutex_lock(&control->mutex)); control->released = 1;
  assert(!pthread_cond_broadcast(&control->condition)); assert(!pthread_mutex_unlock(&control->mutex));
}
static void dispose_control(control_t *control) {
  assert(!pthread_cond_destroy(&control->condition)); assert(!pthread_mutex_destroy(&control->mutex));
}
static void *foreign_join(void *raw) { rinzu_async_join(raw); return NULL; }
int main(int argc, char **argv) {
  rinzu_async_runtime_t runtime; rinzu_async_init(&runtime);
  runtime.record_evidence = 1;
  rinzu_source_position_t source = {1, 2, 3, 1};
  rinzu_async_future_t *first = rinzu_async_worker(&runtime, NULL, NULL, 0, RINZU_SCALAR_U64, slow, source);
  control_t *control = attach(first, 1);
  assert(rinzu_async_admit(first)); rinzu_async_start(first); entered(control);
  assert(first->started && !first->joined && !first->settled);
  assert(!atomic_load_explicit(&first->physical_done, memory_order_acquire));
  if (argc > 1) {
    if (argv[1][0] == 'm') rinzu_async_materialize(first);
    else if (argv[1][0] == 'r') rinzu_async_release(first);
    else { pthread_t thread; assert(!pthread_create(&thread, NULL, foreign_join, first)); assert(!pthread_join(thread, NULL)); }
    return 99;
  }
  /* A second body starts and finishes while the first remains physically
     blocked. The caller prefix controls both starts without awaiting either. */
  rinzu_async_future_t *second = rinzu_async_worker(&runtime, NULL, NULL, 0, RINZU_TRANSFER_UNIT, fast, source);
  assert(rinzu_async_admit(second)); rinzu_async_start(second); rinzu_async_join(second);
  assert(runtime.domains == 2 && runtime.in_flight == 2);
  assert(!rinzu_async_cancel(second)); assert(rinzu_async_materialize(second) == 0 && !second->failure.primary);
  rinzu_async_release(second);
  assert(runtime.domains == 1 && runtime.in_flight == 1);
  assert(rinzu_async_cancel(first)); assert(!rinzu_async_cancel(first));
  assert(atomic_load_explicit(&first->terminal, memory_order_acquire) == RINZU_ASYNC_CANCELLED);
  assert(!first->settled && runtime.domains == 1);
  release(control); rinzu_async_join(first);
  assert(runtime.transport_bytes == 52 && runtime.domains == 1);
  assert(rinzu_async_materialize(first) == 0);
  assert(first->failure.primary == 2 && first->failure.primary_source.file == 1);
  assert(first->failure.head->cause == 1 && first->failure.head->source.file == 9);
  assert(first->failure.tail->cause == 5 && first->failure.tail->source.file == 10);
  assert(rinzu_async_materialize(first) == 0 && !rinzu_async_cancel(first));
  assert(!runtime.domains && !runtime.in_flight && !runtime.transport_bytes);
  dispose_control(control); rinzu_async_release(first);
  first = rinzu_async_worker(&runtime, NULL, NULL, 0, RINZU_TRANSFER_UNIT, never, source);
  assert(rinzu_async_admit(first)); assert(rinzu_async_cancel(first));
  rinzu_async_start(first); assert(!first->started); rinzu_async_join(first);
  assert(runtime.domains == 1 && !first->settled);
  assert(rinzu_async_materialize(first) == 0 && first->failure.primary == 2);
  rinzu_async_release(first);
  /* A service cancellation also waits for its actual native return, then
     discards a late scalar success on the Owner. */
  first = rinzu_async_new(&runtime, RINZU_ASYNC_FILE, RINZU_SCALAR_U64, source);
  first->run = slow; control = attach(first, 0);
  rinzu_async_start(first); entered(control); assert(rinzu_async_timeout(first));
  release(control); rinzu_async_join(first);
  assert(rinzu_async_materialize(first) == 0 && first->failure.primary == 2);
  assert(!first->failure.head); dispose_control(control); rinzu_async_release(first);
  size_t records = 0;
  for (rinzu_async_credit_t *credit = runtime.credits; credit; credit = credit->next) {
    assert(credit->released_at > credit->reserved_at); records += 1;
  }
  assert(records == 5 && runtime.credit_sequence == 10);
  rinzu_async_dispose(&runtime);
  assert(!runtime.records && !runtime.domains && !runtime.in_flight && !runtime.transport_bytes);
}
"#).unwrap();
    let output = Command::new("clang")
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
    let output = Command::new(directory.join("test")).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    for mode in ["materialize-active", "release-active", "foreign-join"] {
        let rejected = Command::new(directory.join("test"))
            .arg(mode)
            .output()
            .unwrap();
        assert_eq!(rejected.status.code(), Some(70), "{mode}: {rejected:?}");
    }
    std::fs::remove_dir_all(directory).unwrap();
}
