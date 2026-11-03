#include "rinzu/source_async.h"
#include <assert.h>
#include <sched.h>
static _Atomic int body_started, entered, proceed, returned;
static rinzu_activity_board_t *evidence;
static void blocked(rinzu_async_future_t *future) {
  atomic_store_explicit(&body_started, 1, memory_order_release);
  while (!atomic_load_explicit(&proceed, memory_order_acquire))
    sched_yield();
  rinzu_activity_board_t *board = rinzu_activity_new();
  rinzu_async_attach_native_board(future, board);
  rinzu_activity_t *native =
      rinzu_activity_prepare_native(board, "worker.blocked", 17, 31, 49, 41, 2);
  rinzu_activity_start(native);
  evidence = board;
  atomic_store_explicit(&entered, 1, memory_order_release);
  while (!atomic_load_explicit(&returned, memory_order_acquire))
    sched_yield();
  rinzu_activity_end(native);
  rinzu_async_detach_native_board(future);
  rinzu_activity_stats_t stats = rinzu_activity_stats(board);
  assert(!stats.active && stats.prepared == stats.ended);
  rinzu_activity_dispose(board);
  future->result_slot = 41;
}
int main(void) {
  alarm(20);
  for (unsigned early = 0; early < 2; early++) {
    atomic_store(&body_started, 0);
    atomic_store(&entered, 0);
    atomic_store(&proceed, 0);
    atomic_store(&returned, 0);
    rinzu_async_runtime_t runtime;
    rinzu_async_init(&runtime);
    rinzu_source_position_t source = {17, 31, 49, 1};
    rinzu_async_future_t *future =
        rinzu_async_worker(&runtime, NULL, NULL, 0, RINZU_SCALAR_U64, blocked, source);
    rinzu_async_start(future);
    while (!atomic_load_explicit(&body_started, memory_order_acquire))
      sched_yield();
    if (early)
      assert(rinzu_async_timeout(future));
    atomic_store_explicit(&proceed, 1, memory_order_release);
    while (!atomic_load_explicit(&entered, memory_order_acquire))
      sched_yield();
    if (!early)
      assert(rinzu_async_cancel(future));
    for (unsigned n = 0; n < 64; n++)
      assert(!rinzu_async_cancel(future));
    assert(rinzu_activity_stats(evidence).active == 2);
    assert(!atomic_load_explicit(&future->physical_done, memory_order_acquire));
    assert(runtime.records == 1 && runtime.domains == 1 && !future->settled);
    atomic_store_explicit(&returned, 1, memory_order_release);
    /* Race late Origin requests against native detachment/disposal. */
    while (!atomic_load_explicit(&future->physical_done, memory_order_acquire))
      rinzu_async_cancel(future);
    rinzu_async_join(future);
    assert(rinzu_async_materialize(future) == 0);
    assert(!future->native_board && future->joined && future->settled);
    rinzu_async_cancel(future);
    rinzu_async_release(future);
    rinzu_async_dispose(&runtime);
  }
  puts("PASS native-only Worker cancellation and physical return");
}
