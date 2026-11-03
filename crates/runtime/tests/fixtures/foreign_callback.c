#include "rinzu/callback_entry.h"
#include "rinzu/foreign_queue.h"
#include "rinzu/owner_transition.h"
#include <assert.h>
#include <limits.h>
#include <sched.h>
#include <stdbool.h>
#include <stdio.h>

/* The actual precompiled entry receives the native caller's packed stack.
   Only Owner decodes it, while that caller is synchronously waiting. */
typedef double (*MixedEntry)(int8_t, float, uint16_t, double, int32_t, uint64_t, const void *,
                             int16_t, uint8_t, int64_t, bool, uint32_t, int8_t, uint16_t,
                             const void *, float, double, float, double, float, double, float,
                             double, float);
static const unsigned char bytes[4] = {0, 255, 7, 0};
static const char string[] = "foreign 日本語";
static const uint64_t result_bits = UINT64_C(0xfff8010203040506);
static const unsigned kinds[24] = {0, 8, 5,  9, 2, 7, 11, 1, 4, 3, 10, 6,
                                   0, 5, 11, 8, 9, 8, 9,  8, 9, 8, 9,  8};
typedef struct {
  pthread_t owner;
  size_t calls;
  uint64_t result;
  rinzu_owner_transition_t *transitions;
} Context;
static double f64(uint64_t bits) {
  double value;
  memcpy(&value, &bits, 8);
  return value;
}
static float f32(uint32_t bits) {
  float value;
  memcpy(&value, &bits, 4);
  return value;
}
static uint64_t call_mixed(const void *pointer) {
  double result = ((MixedEntry)pointer)(
      INT8_MIN, f32(0x80000000), UINT16_MAX, f64(UINT64_C(0x7ff8123456789abc)), INT32_MIN,
      UINT64_MAX, string, INT16_MIN, UINT8_MAX, INT64_MIN, true, UINT32_MAX, -7, 64523, bytes,
      f32(0x7f800000), f64(UINT64_C(0xfff0000000000000)), f32(0x7fc12345),
      f64(UINT64_C(0x8000000000000000)), 0, 42.5, 1.25, 2.5, -2.25);
  uint64_t bits;
  memcpy(&bits, &result, 8);
  return bits;
}
static void owner(Context *context) {
  assert(pthread_equal(context->owner, pthread_self()));
  context->calls++;
}
static uint64_t mixed_handler(void *raw, const rinzu_callback_frame_t *frame) {
  Context *context = raw;
  owner(context);
  rinzu_owner_callback_scope_t scope =
      rinzu_callback_foreign_executing() ? rinzu_owner_admitted_callback_enter(context->transitions)
                                         : rinzu_owner_callback_enter(context->transitions);
  const uint64_t expected[24] = {(uint64_t)(int64_t)INT8_MIN,
                                 0x80000000,
                                 UINT16_MAX,
                                 UINT64_C(0x7ff8123456789abc),
                                 (uint64_t)(int64_t)INT32_MIN,
                                 UINT64_MAX,
                                 (uintptr_t)string,
                                 (uint64_t)(int64_t)INT16_MIN,
                                 UINT8_MAX,
                                 (uint64_t)INT64_MIN,
                                 1,
                                 UINT32_MAX,
                                 (uint64_t)(int64_t)-7,
                                 64523,
                                 (uintptr_t)bytes,
                                 0x7f800000,
                                 UINT64_C(0xfff0000000000000),
                                 0x7fc12345,
                                 UINT64_C(0x8000000000000000),
                                 0,
                                 UINT64_C(0x4045400000000000),
                                 0x3fa00000,
                                 UINT64_C(0x4004000000000000),
                                 0xc0100000};
  rinzu_callback_abi_cursor_t cursor = {0};
  for (size_t index = 0; index < 24; index++)
    assert(rinzu_callback_frame_word(frame, &cursor, kinds[index]) == expected[index]);
  assert(cursor.integers == 8 && cursor.floats == 8 && cursor.stack == 44);
  assert(!strcmp((const char *)(uintptr_t)expected[6], string));
  assert(!memcmp((const void *)(uintptr_t)expected[14], bytes, 4));
  rinzu_owner_callback_leave(scope);
  return result_bits;
}
static uint64_t pointer_handler(void *raw, const unsigned char *input) {
  Context *context = raw;
  owner(context);
  assert(input && !memcmp(input, bytes, 4));
  rinzu_owner_callback_scope_t scope =
      rinzu_callback_foreign_executing() ? rinzu_owner_admitted_callback_enter(context->transitions)
                                         : rinzu_owner_callback_enter(context->transitions);
  rinzu_owner_native_frame_t nested = {0};
  rinzu_owner_native_enter(context->transitions, &nested, 99, 3);
  rinzu_owner_native_leave(context->transitions, &nested);
  rinzu_owner_callback_leave(scope);
  return 41;
}
static uint64_t constant_handler(void *raw, const rinzu_callback_frame_t *frame) {
  assert(frame);
  Context *context = raw;
  owner(context);
  rinzu_owner_callback_scope_t scope = rinzu_owner_admitted_callback_enter(context->transitions);
  rinzu_owner_callback_leave(scope);
  return context->result;
}
static uint64_t rejected_handler(void *raw, const rinzu_callback_frame_t *frame) {
  (void)raw;
  (void)frame;
  puts("BODY executed before rejection");
  fflush(stdout);
  abort();
}
enum { WORKERS = 8, REPEATS = 64 };
typedef struct {
  const void *mixed;
  rinzu_callback_pointer_u64_t pointer;
  size_t repeats;
} Worker;
static void *worker(void *raw) {
  Worker *worker = raw;
  for (size_t index = 0; index < worker->repeats; index++) {
    if (worker->mixed)
      assert(call_mixed(worker->mixed) == result_bits);
    if (worker->pointer)
      assert(worker->pointer(bytes) == 41);
  }
  return NULL;
}
static void drain(rinzu_foreign_queue_t *queue, size_t count) {
  for (size_t completed = 0; completed < count;) {
    if (rinzu_callback_foreign_drain_one(queue))
      completed++;
    else
      sched_yield();
  }
}
static void receipt_baseline(rinzu_foreign_queue_t *queue) {
  rinzu_foreign_stats_t stats = rinzu_foreign_stats(queue);
  assert(!stats.live && !stats.bytes && !stats.queued);
  assert(stats.created == stats.published && stats.created == stats.received &&
         stats.created == stats.retired);
}
typedef struct {
  const void *entry;
  unsigned kind;
  uint64_t bits;
} Result;
static void *result_worker(void *raw) {
  Result *result = raw;
  const void *entry = result->entry;
  uint64_t bits = result->bits;
#define CHECK(type, expected) assert(((type (*)(void))entry)() == (expected))
  switch (result->kind) {
  case 0:
    CHECK(int8_t, INT8_MIN);
    break;
  case 1:
    CHECK(int16_t, INT16_MIN);
    break;
  case 2:
    CHECK(int32_t, INT32_MIN);
    break;
  case 3:
    CHECK(int64_t, INT64_MIN);
    break;
  case 4:
    CHECK(uint8_t, UINT8_MAX);
    break;
  case 5:
    CHECK(uint16_t, UINT16_MAX);
    break;
  case 6:
    CHECK(uint32_t, UINT32_MAX);
    break;
  case 7:
    CHECK(uint64_t, UINT64_MAX);
    break;
  case 8: {
    float value = ((float (*)(void))entry)();
    uint32_t actual;
    memcpy(&actual, &value, 4);
    assert(actual == bits);
    break;
  }
  case 9: {
    double value = ((double (*)(void))entry)();
    uint64_t actual;
    memcpy(&actual, &value, 8);
    assert(actual == bits);
    break;
  }
  case 10:
    CHECK(bool, true);
    break;
  case 11:
    CHECK(const char *, string);
    break;
  case 12:
  case 13:
    CHECK(const unsigned char *, bytes);
    break;
  case 14:
    ((void (*)(void))entry)();
    break;
  default:
    abort();
  }
#undef CHECK
  return NULL;
}
static void *wrong_bind(void *raw) {
  void **values = raw;
  rinzu_callback_entry_bind_foreign(values[0], values[1], 7, 1);
  abort();
}
static void *wrong_diagnostic_bind(void *raw) {
  void **values = raw;
  rinzu_callback_entry_bind_diagnostics(values[0], values[1], "wrong", 17, 31, 49, 41, 2);
  abort();
}
static void diagnostics(int stalled) {
  rinzu_callback_stats_t before = rinzu_callback_entry_stats();
  rinzu_foreign_queue_t *queue = rinzu_foreign_new(NULL, NULL);
  rinzu_foreign_attach_main(queue);
  rinzu_foreign_loop_begin(queue, 7);
  rinzu_owner_transition_t transitions = {0};
  rinzu_owner_transition_init(&transitions);
  rinzu_owner_native_frame_t frame = {0};
  rinzu_owner_native_enter(&transitions, &frame, 7, 11);
  rinzu_activity_board_t *board = rinzu_activity_new();
  uint64_t identity = (uint64_t)(uintptr_t)pthread_self();
  rinzu_activity_t *native = rinzu_activity_prepare(board, RINZU_ACTIVITY_NATIVE, "fixture.native",
                                                    17, 31, 49, identity, 1);
  rinzu_activity_start(native);
  Context context = {.owner = pthread_self(), .transitions = &transitions};
  rinzu_callback_pointer_u64_entry_t callback =
      rinzu_callback_pointer_u64_prepare_deferred(pointer_handler, &context);
  char symbol[] = "fixture.callback";
  rinzu_callback_entry_bind_diagnostics(callback.control, board, symbol, 17, 31, 49, identity, 2);
  rinzu_callback_stats_t running = rinzu_callback_entry_stats();
  assert(running.live_diagnostics == before.live_diagnostics + 1 &&
         running.diagnostic_bytes > before.diagnostic_bytes);
  memset(symbol, '?', sizeof(symbol) - 1);
  rinzu_callback_entry_bind_foreign(callback.control, queue, 7, 1);
  rinzu_callback_entry_open(callback.control);
  rinzu_callback_frame_entry_t abandoned =
      rinzu_callback_frame_prepare_deferred(rejected_handler, &context);
  rinzu_callback_entry_bind_diagnostics(abandoned.control, board, "abandoned", 17, 31, 49, identity,
                                        2);
  assert(rinzu_callback_entry_stats().live_diagnostics == before.live_diagnostics + 2);
  assert(rinzu_callback_entry_close(abandoned.control));
  rinzu_callback_stats_t abandoned_stats = rinzu_callback_entry_stats();
  assert(abandoned_stats.live_diagnostics == running.live_diagnostics &&
         abandoned_stats.diagnostic_bytes == running.diagnostic_bytes);
  assert(abandoned_stats.closed_entries == before.closed_entries + 1 &&
         abandoned_stats.tombstone_bytes > before.tombstone_bytes);
  Worker work = {NULL, callback.entry, 1};
  pthread_t thread;
  assert(!pthread_create(&thread, NULL, worker, &work));
  while (!rinzu_foreign_stats(queue).queued)
    sched_yield();
  struct timespec duration = {stalled ? 1 : 0, stalled ? 150000000L : 160000000L};
  assert(!nanosleep(&duration, NULL));
  rinzu_activity_stats_t stats = rinzu_activity_stats(board);
  assert(stats.active == 3 && stats.warnings[RINZU_ACTIVITY_FOREIGN_WAIT] == 1);
  assert(stats.warnings[RINZU_ACTIVITY_OWNER_STALL] == (unsigned)stalled);
  rinzu_foreign_loop_end(queue, 7);
  rinzu_owner_native_leave(&transitions, &frame);
  rinzu_activity_end(native);
  assert(!rinzu_callback_entry_close(callback.control));
  assert(rinzu_callback_entry_stats().live_diagnostics == running.live_diagnostics);
  drain(queue, 1);
  while (!rinzu_callback_entry_settled(callback.control))
    sched_yield();
  stats = rinzu_activity_stats(board);
  assert(!stats.active && stats.prepared == stats.ended);
  rinzu_callback_stats_t settled = rinzu_callback_entry_stats();
  assert(settled.live_diagnostics == before.live_diagnostics &&
         settled.diagnostic_bytes == before.diagnostic_bytes);
  assert(settled.closed_entries == before.closed_entries + 2 &&
         settled.tombstone_bytes > abandoned_stats.tombstone_bytes);
  assert(settled.entries == before.entries + 2 &&
         settled.control_bytes >=
             before.control_bytes + settled.tombstone_bytes - before.tombstone_bytes);
  /* The interval ends before receipt. Ready metadata itself is pinned by the
     invocation through its final native record retirement. */
  while (rinzu_foreign_stats(queue).live)
    sched_yield();
  rinzu_activity_dispose(board);
  assert(!pthread_join(thread, NULL));
  receipt_baseline(queue);
  assert(context.calls == 1);
  rinzu_owner_transition_finish(&transitions);
  rinzu_foreign_dispose(queue);
  puts("PASS foreign wait metadata copy and close/receipt settlement");
}
int main(int argc, char **argv) {
  alarm(20);
  const char *mode = argc > 1 ? argv[1] : "normal";
  if (!strcmp(mode, "diagnostics") || !strcmp(mode, "stall")) {
    diagnostics(!strcmp(mode, "stall"));
    return 0;
  }
  rinzu_foreign_queue_t *queue = rinzu_foreign_new(NULL, NULL);
  Context context = {.owner = pthread_self()};
  if (strcmp(mode, "worker") != 0)
    rinzu_foreign_attach_main(queue);
  rinzu_callback_frame_entry_t rejected =
      rinzu_callback_frame_prepare_deferred(rejected_handler, &context);
  if (!strncmp(mode, "diagnostic-", 11)) {
    rinzu_activity_board_t *board = rinzu_activity_new();
    if (!strcmp(mode, "diagnostic-foreign")) {
      void *values[] = {rejected.control, board};
      pthread_t thread;
      assert(!pthread_create(&thread, NULL, wrong_diagnostic_bind, values));
      assert(!pthread_join(thread, NULL));
      abort();
    }
    if (!strcmp(mode, "diagnostic-oom"))
      rinzu_callback_test_fail_allocation_after(0);
    rinzu_callback_entry_bind_diagnostics(rejected.control, board, "prepared", 17, 31, 49, 41, 2);
    if (!strcmp(mode, "diagnostic-open"))
      rinzu_callback_entry_open(rejected.control);
    rinzu_callback_entry_bind_diagnostics(rejected.control, board, "duplicate", 17, 31, 49, 41, 2);
    abort();
  }
  if (!strcmp(mode, "foreign-bind")) {
    void *values[] = {rejected.control, queue};
    pthread_t thread;
    assert(!pthread_create(&thread, NULL, wrong_bind, values));
    assert(!pthread_join(thread, NULL));
    abort();
  }
  rinzu_callback_entry_bind_foreign(rejected.control, queue, 7, strcmp(mode, "unsupported") != 0);
  if (!strcmp(mode, "duplicate-bind")) {
    rinzu_callback_entry_bind_foreign(rejected.control, queue, 7, 1);
    abort();
  }
  if (strcmp(mode, "prepared") != 0)
    rinzu_callback_entry_open(rejected.control);
  if (!strcmp(mode, "late-bind")) {
    rinzu_callback_entry_bind_foreign(rejected.control, queue, 7, 1);
    abort();
  }
  if (strcmp(mode, "worker") != 0 && strcmp(mode, "ordinary") != 0)
    rinzu_foreign_loop_begin(queue, !strcmp(mode, "stale-generation") ? 8 : 7);
  if (!strcmp(mode, "bounded"))
    rinzu_foreign_bounded_enter(queue);
  if (!strcmp(mode, "closed"))
    assert(rinzu_callback_entry_close(rejected.control));
  if (strcmp(mode, "normal") != 0) {
    Result result = {rejected.entry, 14, 0};
    pthread_t thread;
    assert(!pthread_create(&thread, NULL, result_worker, &result));
    assert(!pthread_join(thread, NULL));
    abort();
  }
  assert(rinzu_callback_entry_close(rejected.control));
  rinzu_owner_transition_t transitions = {0};
  rinzu_owner_transition_init(&transitions);
  rinzu_owner_native_frame_t native = {0};
  rinzu_owner_native_enter(&transitions, &native, 7, 11);
  Context mixed = {.owner = pthread_self(), .transitions = &transitions},
          pointer = {.owner = pthread_self(), .transitions = &transitions};
  rinzu_callback_frame_entry_t frame = rinzu_callback_frame_prepare_deferred(mixed_handler, &mixed);
  rinzu_callback_pointer_u64_entry_t ptr =
      rinzu_callback_pointer_u64_prepare_deferred(pointer_handler, &pointer);
  rinzu_callback_entry_bind_foreign(frame.control, queue, 7, 1);
  rinzu_callback_entry_bind_foreign(ptr.control, queue, 7, 1);
  rinzu_callback_entry_open(frame.control);
  rinzu_callback_entry_open(ptr.control);
  /* Same-thread nested bounded calls continue to use the direct path. */
  rinzu_foreign_bounded_enter(queue);
  assert(call_mixed(frame.entry) == result_bits);
  assert(ptr.entry(bytes) == 41);
  rinzu_foreign_bounded_leave(queue);
  assert(!rinzu_foreign_stats(queue).created);
  pthread_t threads[WORKERS];
  Worker work = {frame.entry, ptr.entry, REPEATS};
  for (size_t index = 0; index < WORKERS; index++)
    assert(!pthread_create(&threads[index], NULL, worker, &work));
  drain(queue, WORKERS * REPEATS * 2);
  for (size_t index = 0; index < WORKERS; index++)
    assert(!pthread_join(threads[index], NULL));
  assert(mixed.calls == WORKERS * REPEATS + 1 && pointer.calls == WORKERS * REPEATS + 1);
  assert(rinzu_callback_entry_close(frame.control));
  assert(rinzu_callback_entry_close(ptr.control));
  receipt_baseline(queue);
  uint64_t bits[15] = {(uint64_t)(int64_t)INT8_MIN,
                       (uint64_t)(int64_t)INT16_MIN,
                       (uint64_t)(int64_t)INT32_MIN,
                       (uint64_t)INT64_MIN,
                       UINT8_MAX,
                       UINT16_MAX,
                       UINT32_MAX,
                       UINT64_MAX,
                       0xffc12345,
                       result_bits,
                       1,
                       (uintptr_t)string,
                       (uintptr_t)bytes,
                       (uintptr_t)bytes,
                       0};
  for (unsigned kind = 0; kind < 15; kind++) {
    Context value = {.owner = pthread_self(), .result = bits[kind], .transitions = &transitions};
    frame = rinzu_callback_frame_prepare_deferred(constant_handler, &value);
    rinzu_callback_entry_bind_foreign(frame.control, queue, 7, 1);
    rinzu_callback_entry_open(frame.control);
    Result result = {frame.entry, kind, bits[kind]};
    pthread_t thread;
    assert(!pthread_create(&thread, NULL, result_worker, &result));
    drain(queue, 1);
    assert(!pthread_join(thread, NULL));
    assert(value.calls == 1);
    assert(rinzu_callback_entry_close(frame.control));
    receipt_baseline(queue);
  }
  /* Already admitted records remain authorized after physical return/close.
     Root/context connection is retained until all native receipts settle. */
  ptr = rinzu_callback_pointer_u64_prepare_deferred(pointer_handler, &pointer);
  rinzu_callback_entry_bind_foreign(ptr.control, queue, 7, 1);
  rinzu_callback_entry_open(ptr.control);
  work = (Worker){NULL, ptr.entry, 1};
  for (size_t index = 0; index < WORKERS; index++)
    assert(!pthread_create(&threads[index], NULL, worker, &work));
  while (rinzu_foreign_stats(queue).queued != WORKERS)
    sched_yield();
  rinzu_foreign_loop_end(queue, 7);
  rinzu_owner_native_leave(&transitions, &native);
  assert(!rinzu_callback_entry_close(ptr.control));
  assert(!rinzu_callback_entry_settled(ptr.control));
  assert(rinzu_callback_entry_stats().in_flight == WORKERS);
  drain(queue, WORKERS);
  for (size_t index = 0; index < WORKERS; index++)
    assert(!pthread_join(threads[index], NULL));
  assert(rinzu_callback_entry_settled(ptr.control));
  receipt_baseline(queue);
  assert(transitions.state == RINZU_OWNER_MANAGED && !transitions.current);
  rinzu_owner_transition_finish(&transitions);
  rinzu_callback_stats_t callbacks = rinzu_callback_entry_stats();
  assert(!callbacks.live_connections && !callbacks.in_flight);
  assert(callbacks.invocations == 2 + WORKERS * REPEATS * 2 + 15 + WORKERS);
  assert(rinzu_foreign_stats(queue).created == WORKERS * REPEATS * 2 + 15 + WORKERS);
  rinzu_foreign_dispose(queue);
  puts("PASS foreign callback signature/progress and 1047 actual admitted entries");
}
