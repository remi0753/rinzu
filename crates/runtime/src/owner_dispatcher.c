#include "rinzu/owner_dispatcher.h"
#include "rinzu/callback_entry.h"
#include <CoreFoundation/CoreFoundation.h>
#include <stdbool.h>

struct rinzu_owner_dispatcher {
  pthread_mutex_t gate;
  rinzu_foreign_queue_t *queue;
  CFRunLoopRef loop;
  CFRunLoopSourceRef source;
  CFRunLoopObserverRef observer;
  rinzu_owner_step_t step;
  rinzu_owner_ready_t ready;
  rinzu_owner_enter_t can_enter;
  void *context;
  size_t loop_depth;
  unsigned pending, active, next_foreign;
  rinzu_owner_dispatcher_stats_t stats;
#ifdef RINZU_DISPATCHER_TESTING
  size_t return_after_published;
  unsigned test_stopped;
#endif
};
#ifdef RINZU_DISPATCHER_TESTING
static rinzu_foreign_notify_t empty_hook;
static void *empty_context;
void rinzu_owner_dispatcher_test_empty_hook(rinzu_foreign_notify_t hook, void *context) {
  empty_hook = hook;
  empty_context = context;
}
#endif
static void owner(rinzu_owner_dispatcher_t *dispatcher) {
  if (!dispatcher)
    _Exit(86);
  rinzu_foreign_owner(dispatcher->queue);
}
#ifdef RINZU_DISPATCHER_TESTING
void rinzu_owner_dispatcher_test_return_after_published(rinzu_owner_dispatcher_t *dispatcher,
                                                        size_t count) {
  owner(dispatcher);
  rinzu_foreign_lock(&dispatcher->queue->gate);
  if (!count || dispatcher->queue->loop_generation || dispatcher->queue->stats.live)
    _Exit(86);
  rinzu_foreign_unlock(&dispatcher->queue->gate);
  dispatcher->return_after_published = count;
}
#endif
void rinzu_owner_dispatcher_wake(void *raw) {
  rinzu_owner_dispatcher_t *dispatcher = raw;
  if (!dispatcher)
    _Exit(86);
  rinzu_foreign_lock(&dispatcher->gate);
  if (dispatcher->stats.wakes == UINT64_MAX)
    _Exit(71);
  dispatcher->stats.wakes++;
  int signal = !dispatcher->pending;
  dispatcher->pending = 1;
  if (signal) {
    if (dispatcher->stats.signals == UINT64_MAX)
      _Exit(71);
    dispatcher->stats.signals++;
  }
  rinzu_foreign_unlock(&dispatcher->gate);
  if (signal) {
    CFRunLoopSourceSignal(dispatcher->source);
    CFRunLoopWakeUp(dispatcher->loop);
  }
}
static int managed(rinzu_owner_dispatcher_t *dispatcher, unsigned lane) {
  return dispatcher->step && dispatcher->step(dispatcher->context, lane);
}
static int ready(rinzu_owner_dispatcher_t *dispatcher) {
  return rinzu_foreign_stats(dispatcher->queue).queued ||
         (dispatcher->ready &&
          (dispatcher->ready(dispatcher->context, 0) || dispatcher->ready(dispatcher->context, 1)));
}
static uint64_t elapsed(struct timespec start, struct timespec end) {
  if (end.tv_sec < start.tv_sec || (end.tv_sec == start.tv_sec && end.tv_nsec < start.tv_nsec))
    _Exit(86);
  uint64_t seconds = (uint64_t)(end.tv_sec - start.tv_sec);
  if (seconds > UINT64_MAX / UINT64_C(1000000000))
    return UINT64_MAX;
  return seconds * UINT64_C(1000000000) + (uint64_t)(end.tv_nsec + 1000000000L - start.tv_nsec) -
         UINT64_C(1000000000);
}
void rinzu_owner_dispatcher_pump(rinzu_owner_dispatcher_t *dispatcher) {
  owner(dispatcher);
  rinzu_foreign_lock(&dispatcher->gate);
  dispatcher->pending = 0;
  if (dispatcher->active) {
    dispatcher->stats.deferred++;
    rinzu_foreign_unlock(&dispatcher->gate);
    return;
  }
  rinzu_foreign_unlock(&dispatcher->gate);
  rinzu_foreign_lock(&dispatcher->queue->gate);
  int bounded = dispatcher->queue->bounded_depth != 0;
  rinzu_foreign_unlock(&dispatcher->queue->gate);
  if (bounded) {
    rinzu_foreign_lock(&dispatcher->gate);
    dispatcher->stats.deferred++;
    rinzu_foreign_unlock(&dispatcher->gate);
    return;
  }
  if (dispatcher->can_enter && !dispatcher->can_enter(dispatcher->context)) {
    rinzu_foreign_lock(&dispatcher->gate);
    dispatcher->stats.deferred++;
    rinzu_foreign_unlock(&dispatcher->gate);
    return;
  }
  rinzu_foreign_lock(&dispatcher->gate);
  dispatcher->active = 1;
  dispatcher->stats.turns++;
  rinzu_foreign_unlock(&dispatcher->gate);
  struct timespec start, now;
  rinzu_foreign_time(&start);
  size_t count = 0, foreign = 0, tasks = 0;
  while (count < 64) {
    int did = managed(dispatcher, 0);
    if (did)
      tasks++;
    else {
      if (dispatcher->next_foreign) {
        did = rinzu_callback_foreign_drain_one(dispatcher->queue);
        if (did)
          foreign++;
        else {
          did = managed(dispatcher, 1);
          if (did)
            tasks++;
        }
      } else {
        did = managed(dispatcher, 1);
        if (did)
          tasks++;
        else {
          did = rinzu_callback_foreign_drain_one(dispatcher->queue);
          if (did)
            foreign++;
        }
      }
      dispatcher->next_foreign = !dispatcher->next_foreign;
    }
    if (!did)
      break;
    count++;
    rinzu_foreign_time(&now);
    if (elapsed(start, now) >= UINT64_C(2000000))
      break;
  }
  /* Clear pending before draining, never after an empty snapshot. A concurrent
     publisher either supplies the next signal or observes our rearm. */
  int more = ready(dispatcher);
#ifdef RINZU_DISPATCHER_TESTING
  if (!more && empty_hook)
    empty_hook(empty_context);
#endif
  rinzu_foreign_lock(&dispatcher->gate);
  dispatcher->active = 0;
  dispatcher->stats.activations += count;
  dispatcher->stats.foreign += foreign;
  dispatcher->stats.managed += tasks;
  if (count > dispatcher->stats.max_activations)
    dispatcher->stats.max_activations = count;
  if (more)
    dispatcher->stats.rearms++;
  rinzu_foreign_unlock(&dispatcher->gate);
  if (more)
    rinzu_owner_dispatcher_wake(dispatcher);
}
static void perform(void *raw) {
  rinzu_owner_dispatcher_t *dispatcher = raw;
  owner(dispatcher);
  /* A source in an unsupported/nested modal loop cannot interrupt managed. */
  if (dispatcher->loop_depth != 1) {
    rinzu_foreign_lock(&dispatcher->gate);
    dispatcher->pending = 0;
    rinzu_foreign_unlock(&dispatcher->gate);
    return;
  }
#ifdef RINZU_DISPATCHER_TESTING
  /* Hold only RunLoop delivery. Real post-return settlement still uses pump.
     Publication, admission, close, decoding and receipts are production code. */
  if (dispatcher->return_after_published) {
    rinzu_foreign_lock(&dispatcher->gate);
    dispatcher->pending = 0;
    rinzu_foreign_unlock(&dispatcher->gate);
    if (!dispatcher->test_stopped &&
        rinzu_foreign_stats(dispatcher->queue).queued >= dispatcher->return_after_published) {
      dispatcher->test_stopped = 1;
      CFRunLoopStop(dispatcher->loop);
    }
    return;
  }
#endif
  rinzu_owner_dispatcher_pump(dispatcher);
}
static void observe(CFRunLoopObserverRef observer, CFRunLoopActivity activity, void *raw) {
  (void)observer;
  rinzu_owner_dispatcher_t *dispatcher = raw;
  owner(dispatcher);
  if (activity == kCFRunLoopEntry) {
    if (dispatcher->loop_depth == SIZE_MAX)
      _Exit(71);
    dispatcher->loop_depth++;
  } else if (activity == kCFRunLoopExit) {
    if (!dispatcher->loop_depth)
      _Exit(86);
    dispatcher->loop_depth--;
  }
  CFStringRef mode = CFRunLoopCopyCurrentMode(dispatcher->loop);
  int allowed = mode && CFRunLoopContainsSource(dispatcher->loop, dispatcher->source, mode);
  if (mode)
    CFRelease(mode);
  rinzu_foreign_queue_t *queue = dispatcher->queue;
  rinzu_foreign_lock(&queue->gate);
  queue->runloop_depth = dispatcher->loop_depth;
  queue->runloop_allowed = allowed;
  rinzu_foreign_unlock(&queue->gate);
  /* Restoring the outer mode after a nested exit makes retained work runnable.
     Deferred wake is explicitly rearmed because the source may have consumed
     its signal while a managed/native activation prevented a turn. */
  if (dispatcher->loop_depth == 1 && allowed && ready(dispatcher)) {
    rinzu_owner_dispatcher_wake(dispatcher);
  }
}
rinzu_owner_dispatcher_t *rinzu_owner_dispatcher_new(rinzu_foreign_queue_t *queue,
                                                     rinzu_owner_step_t step,
                                                     rinzu_owner_ready_t has_ready,
                                                     rinzu_owner_enter_t can_enter, void *context) {
  rinzu_foreign_owner(queue);
  if ((step && !has_ready) || (!step && has_ready))
    _Exit(86);
  rinzu_owner_dispatcher_t *dispatcher = rinzu_foreign_allocate(sizeof(*dispatcher));
  rinzu_foreign_sync(pthread_mutex_init(&dispatcher->gate, NULL));
  dispatcher->queue = queue;
  dispatcher->step = step;
  dispatcher->ready = has_ready;
  dispatcher->can_enter = can_enter;
  dispatcher->context = context;
  dispatcher->next_foreign = 1;
  dispatcher->loop = CFRunLoopGetCurrent();
  if (!dispatcher->loop)
    _Exit(71);
  CFRetain(dispatcher->loop);
  CFRunLoopSourceContext source = {.info = dispatcher, .perform = perform};
  dispatcher->source = CFRunLoopSourceCreate(NULL, 0, &source);
  if (!dispatcher->source)
    _Exit(71);
  CFRunLoopObserverContext observer = {.info = dispatcher};
  dispatcher->observer =
      CFRunLoopObserverCreate(NULL, kCFRunLoopAllActivities, true, 0, observe, &observer);
  if (!dispatcher->observer)
    _Exit(71);
  rinzu_foreign_lock(&queue->gate);
  if (!queue->main_domain || queue->loop_generation || queue->stats.live ||
      queue->dispatcher_attached || queue->wake)
    _Exit(86);
  queue->dispatcher_attached = 1;
  queue->wake = rinzu_owner_dispatcher_wake;
  queue->wake_context = dispatcher;
  rinzu_foreign_unlock(&queue->gate);
  CFRunLoopAddSource(dispatcher->loop, dispatcher->source, kCFRunLoopDefaultMode);
  CFRunLoopAddSource(dispatcher->loop, dispatcher->source, kCFRunLoopCommonModes);
  CFRunLoopAddObserver(dispatcher->loop, dispatcher->observer, kCFRunLoopCommonModes);
  /* Known modes also get the observer, even when they are not compatible. */
  CFArrayRef modes = CFRunLoopCopyAllModes(dispatcher->loop);
  if (!modes)
    _Exit(71);
  for (CFIndex index = 0; index < CFArrayGetCount(modes); index++)
    CFRunLoopAddObserver(dispatcher->loop, dispatcher->observer,
                         CFArrayGetValueAtIndex(modes, index));
  CFRelease(modes);
  return dispatcher;
}
rinzu_owner_dispatcher_stats_t rinzu_owner_dispatcher_stats(rinzu_owner_dispatcher_t *dispatcher) {
  owner(dispatcher);
  rinzu_foreign_lock(&dispatcher->gate);
  rinzu_owner_dispatcher_stats_t stats = dispatcher->stats;
  rinzu_foreign_unlock(&dispatcher->gate);
  return stats;
}
void rinzu_owner_dispatcher_set_hooks(rinzu_owner_dispatcher_t *dispatcher, rinzu_owner_step_t step,
                                      rinzu_owner_ready_t has_ready, rinzu_owner_enter_t can_enter,
                                      void *context) {
  owner(dispatcher);
  if (!step || !has_ready || !can_enter || !context || dispatcher->step || dispatcher->ready ||
      dispatcher->can_enter || dispatcher->context || dispatcher->active || dispatcher->loop_depth)
    _Exit(86);
  rinzu_foreign_lock(&dispatcher->queue->gate);
  if (dispatcher->queue->loop_generation || dispatcher->queue->stats.live)
    _Exit(86);
  rinzu_foreign_unlock(&dispatcher->queue->gate);
  dispatcher->step = step;
  dispatcher->ready = has_ready;
  dispatcher->can_enter = can_enter;
  dispatcher->context = context;
}
void rinzu_owner_dispatcher_dispose(rinzu_owner_dispatcher_t *dispatcher) {
  owner(dispatcher);
  rinzu_foreign_queue_t *queue = dispatcher->queue;
  rinzu_foreign_lock(&queue->gate);
  if (queue->loop_generation || queue->stats.live || queue->stats.wake_pins ||
      queue->bounded_depth || dispatcher->loop_depth || dispatcher->active)
    _Exit(86);
  queue->wake = NULL;
  queue->wake_context = NULL;
  queue->dispatcher_attached = 0;
  queue->runloop_depth = 0;
  queue->runloop_allowed = 0;
  rinzu_foreign_unlock(&queue->gate);
  CFRunLoopSourceInvalidate(dispatcher->source);
  CFRunLoopObserverInvalidate(dispatcher->observer);
  CFRelease(dispatcher->source);
  CFRelease(dispatcher->observer);
  CFRelease(dispatcher->loop);
  rinzu_foreign_sync(pthread_mutex_destroy(&dispatcher->gate));
  free(dispatcher);
}
