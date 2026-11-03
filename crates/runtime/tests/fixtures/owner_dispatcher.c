#include "rinzu/owner_dispatcher.h"
#include "rinzu/callback_entry.h"
#include <CoreFoundation/CoreFoundation.h>
#include <assert.h>
#include <sched.h>
#include <stdio.h>

enum { WORKERS = 8, REPEATS = 128 };
typedef struct {
  pthread_t owner;
  rinzu_foreign_queue_t *queue;
  rinzu_owner_dispatcher_t *dispatcher;
  rinzu_callback_pointer_u64_entry_t callback;
  size_t control, ready, handled, ticks, nested;
  _Atomic size_t finished;
  _Atomic size_t race_stage;
  size_t expected_workers;
  int negative, slow, lost_wake;
} Context;
static uint64_t handler(void *raw, const unsigned char *input) {
  Context *context = raw;
  assert(pthread_equal(context->owner, pthread_self()));
  assert(!context->control && input && input[0] == 42);
  context->handled++;
  return 41;
}
static void *worker(void *raw) {
  Context *context = raw;
  const unsigned char input[4] = {42, 0, 255, 0};
  while (!rinzu_foreign_can_admit(context->queue, 7))
    sched_yield();
  for (size_t index = 0; index < REPEATS; index++)
    assert(context->callback.entry(input) == 41);
  atomic_fetch_add(&context->finished, 1);
  return NULL;
}
static void *rejected(void *raw) {
  Context *context = raw;
  const unsigned char input[4] = {42, 0, 255, 0};
  (void)context->callback.entry(input);
  abort();
}
static void *race_worker(void *raw) {
  Context *context = raw;
  const unsigned char input[4] = {42, 0, 255, 0};
  for (size_t index = 0; index < REPEATS; index++) {
    while (atomic_load(&context->race_stage) <= index)
      sched_yield();
    assert(context->callback.entry(input) == 41);
  }
  atomic_fetch_add(&context->finished, 1);
  return NULL;
}
static void empty_race(void *raw) {
  Context *context = raw;
  if (atomic_load(&context->race_stage) >= REPEATS)
    return;
  atomic_fetch_add(&context->race_stage, 1);
  /* Force publication after the empty snapshot but before the turn exits. */
  while (!rinzu_foreign_stats(context->queue).queued)
    sched_yield();
}
static void reject(Context *context) {
  pthread_t thread;
  assert(!pthread_create(&thread, NULL, rejected, context));
  assert(!pthread_join(thread, NULL));
  abort();
}
static int step(void *raw, unsigned lane) {
  Context *context = raw;
  assert(pthread_equal(context->owner, pthread_self()));
  if (lane == 0) {
    if (!context->control)
      return 0;
    context->control--;
    return 1;
  }
  assert(lane == 1);
  if (!context->ready)
    return 0;
  context->ready--;
  if (context->slow) {
    struct timespec delay = {0, 5000000};
    assert(!nanosleep(&delay, NULL));
  }
  /* Reentrant RunLoop/source turns cannot enter a second managed activation. */
  if (!context->nested++)
    rinzu_owner_dispatcher_pump(context->dispatcher);
  return 1;
}
static int ready(void *raw, unsigned lane) {
  Context *context = raw;
  return lane == 0 ? context->control != 0 : context->ready != 0;
}
static void tick(CFRunLoopTimerRef timer, void *raw) {
  Context *context = raw;
  context->ticks++;
  if (context->negative == 1) {
    rinzu_foreign_bounded_enter(context->queue);
    reject(context);
  }
  if (context->negative == 2)
    reject(context);
  if (context->negative == 3) {
    /* The inner RunLoop observer publishes depth two before its timer fires. */
    context->negative = 2;
    CFRunLoopTimerContext inner_context = {.info = context};
    CFRunLoopTimerRef inner = CFRunLoopTimerCreate(NULL, CFAbsoluteTimeGetCurrent() + 0.001, 0, 0,
                                                   0, tick, &inner_context);
    assert(inner);
    CFRunLoopAddTimer(CFRunLoopGetCurrent(), inner, kCFRunLoopDefaultMode);
    (void)CFRunLoopRunInMode(kCFRunLoopDefaultMode, 1, 0);
    abort();
  }
  rinzu_foreign_stats_t stats = rinzu_foreign_stats(context->queue);
  if (atomic_load(&context->finished) == context->expected_workers && !stats.live &&
      !stats.wake_pins && !context->ready && !context->control) {
    CFRunLoopTimerInvalidate(timer);
    CFRunLoopStop(CFRunLoopGetCurrent());
  }
}
int main(int argc, char **argv) {
  alarm(20);
  const char *mode = argc > 1 ? argv[1] : "normal";
  CFRunLoopRef loop = CFRunLoopGetCurrent();
  CFStringRef custom = CFSTR("rinzu-test-common"), unsupported = CFSTR("rinzu-test-unsupported");
  CFRunLoopAddCommonMode(loop, custom);
  Context context = {
      .owner = pthread_self(), .control = 9, .ready = 4096, .expected_workers = WORKERS};
  atomic_init(&context.finished, 0);
  atomic_init(&context.race_stage, 0);
  if (!strcmp(mode, "budget")) {
    context.control = 0;
    context.ready = 2;
    context.expected_workers = 0;
    context.slow = 1;
  }
  if (!strcmp(mode, "lost-wake")) {
    context.control = 0;
    context.ready = 0;
    context.expected_workers = 1;
    context.lost_wake = 1;
  }
  CFRunLoopTimerContext timer_context = {.info = &context};
  CFRunLoopTimerRef timer = CFRunLoopTimerCreate(NULL, CFAbsoluteTimeGetCurrent() + 0.001, 0.001, 0,
                                                 0, tick, &timer_context);
  assert(timer);
  CFRunLoopAddTimer(loop, timer, kCFRunLoopCommonModes);
  /* Introduce an observed but unsupported mode before runtime registration. */
  CFRunLoopAddTimer(loop, timer, unsupported);
  context.queue = rinzu_foreign_new(NULL, NULL);
  rinzu_foreign_attach_main(context.queue);
  context.dispatcher = rinzu_owner_dispatcher_new(context.queue, step, ready, NULL, &context);
  if (context.lost_wake)
    rinzu_owner_dispatcher_test_empty_hook(empty_race, &context);
  context.callback = rinzu_callback_pointer_u64_prepare_deferred(handler, &context);
  rinzu_callback_entry_bind_foreign(context.callback.control, context.queue, 7, 1);
  rinzu_foreign_loop_begin(context.queue, 7);
  rinzu_callback_entry_open(context.callback.control);
  if (!strcmp(mode, "bounded"))
    context.negative = 1;
  if (!strcmp(mode, "unknown-mode"))
    context.negative = 2;
  if (!strcmp(mode, "nested"))
    context.negative = 3;
  for (size_t index = 0; index < 1000; index++)
    rinzu_owner_dispatcher_wake(context.dispatcher);
  assert(rinzu_owner_dispatcher_stats(context.dispatcher).signals == 1);
  pthread_t threads[WORKERS];
  if (!context.negative)
    for (size_t index = 0; index < context.expected_workers; index++)
      assert(!pthread_create(&threads[index], NULL, context.lost_wake ? race_worker : worker,
                             &context));
  CFStringRef active = !strcmp(mode, "common-mode")
                           ? custom
                           : (!strcmp(mode, "unknown-mode") ? unsupported : kCFRunLoopDefaultMode);
  (void)CFRunLoopRunInMode(active, 10, 0);
  assert(!context.negative);
  for (size_t index = 0; index < context.expected_workers; index++)
    assert(!pthread_join(threads[index], NULL));
  rinzu_foreign_loop_end(context.queue, 7);
  assert(rinzu_callback_entry_close(context.callback.control));
  rinzu_owner_dispatcher_stats_t stats = rinzu_owner_dispatcher_stats(context.dispatcher);
  size_t expected_foreign = context.expected_workers * REPEATS;
  assert(context.handled == expected_foreign && !context.control && !context.ready &&
         context.ticks);
  assert(stats.foreign == expected_foreign && stats.max_activations <= 64);
  if (context.slow) {
    assert(stats.activations == 2 && stats.managed == 2 && stats.max_activations == 1 &&
           stats.turns >= 2);
  } else if (context.lost_wake) {
    assert(stats.activations == REPEATS && !stats.managed &&
           atomic_load(&context.race_stage) == REPEATS);
  } else {
    assert(stats.activations == WORKERS * REPEATS + 4096 + 9 && stats.managed == 4096 + 9 &&
           stats.rearms && stats.deferred);
  }
  assert(stats.signals < stats.wakes);
  rinzu_foreign_stats_t records = rinzu_foreign_stats(context.queue);
  assert(!records.live && !records.bytes && !records.queued && !records.wake_pins);
  assert(records.created == expected_foreign && records.retired == records.created);
  CFRelease(timer);
  rinzu_owner_dispatcher_dispose(context.dispatcher);
  rinzu_foreign_dispose(context.queue);
  puts("PASS runtime-owned RunLoop 1024 callbacks/4096 ready/9 control bounded turns");
}
