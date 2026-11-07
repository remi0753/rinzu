#include "rinzu/managed_data.h"
#include "rinzu/owner_transition.h"
#include <assert.h>
#include <limits.h>
#include <sys/wait.h>

/* Expose the fields used by the actual generated helpers. Full generated
   contexts and native/async transitions are exercised by the AOT tests. */
typedef struct {
  int explicit_shutdown;
} TestAsync;
typedef struct {
  _Atomic int cancel_requested;
} TestWorker;
typedef struct {
  TestAsync *async;
  TestWorker *worker;
  rinzu_owner_transition_t transitions, *shared_transitions;
  rinzu_failure_state_t pending;
  rinzu_error_trace_stack_t error_trace_stack;
  rinzu_managed_roots_t roots;
  rinzu_managed_data_t data;
  rinzu_source_position_t source;
  uint64_t polls, *domain_polls, cancel_after, shutdown_after, callback_depth;
  int cancel_requested, shutdown_requested, cancel_masked;
  unsigned shutdown_checks, cancel_checks;
} RinzuContext;
static rinzu_managed_data_t *rinzu_context_data(RinzuContext *context) {
  return &context->data;
}
static rinzu_owner_transition_t *rinzu_context_transitions(RinzuContext *context) {
  rinzu_owner_transition_init(&context->transitions);
  return &context->transitions;
}
static int rinzu_aot_async_root_shutdown(RinzuContext *context) {
  ++context->shutdown_checks;
  return context->async && context->async->explicit_shutdown;
}
static int rinzu_aot_async_cancelled(RinzuContext *context) {
  ++context->cancel_checks;
  return context->cancel_requested ||
         (context->worker &&
          atomic_load_explicit(&context->worker->cancel_requested, memory_order_acquire));
}
static const char *rinzu_error_trace_function_name(int64_t function) {
  (void)function;
  return "fixture";
}
#include "../../src/aot_safepoint.c"

static void same_source(rinzu_source_position_t a, rinzu_source_position_t b) {
  assert(a.file == b.file && a.start_byte == b.start_byte && a.end_byte == b.end_byte &&
         a.present == b.present);
}
static void packed_source(RinzuContext *context, rinzu_source_position_t source) {
  rinzu_baseline_set_source_packed(context, source.file,
                                   (uint64_t)source.start_byte | ((uint64_t)source.end_byte << 32));
}
static void check_source(void) {
  const int64_t values[] = {
      INT64_MIN, -1, 0, 1, UINT32_MAX - 1, UINT32_MAX, (int64_t)UINT32_MAX + 1, INT64_MAX};
  RinzuContext context = {0};
  for (size_t f = 0; f < 8; ++f)
    for (size_t s = 0; s < 8; ++s)
      for (size_t e = 0; e < 8; ++e) {
        int64_t file = values[f], start = values[s], end = values[e];
        int valid =
            !(file < 0 || file > UINT32_MAX || start < 0 || end < start || end > UINT32_MAX);
        assert(valid == rinzu_aot_source_range_valid(file, start, end));
        if (valid) {
          rinzu_baseline_set_source(&context, file, start, end);
          same_source(context.source, (rinzu_source_position_t){file, start, end, 1});
          packed_source(&context, (rinzu_source_position_t){file, start, end, 1});
          same_source(context.source, (rinzu_source_position_t){file, start, end, 1});
        }
      }
  /* A callee may overwrite the position; restoring the caller is still required. */
  rinzu_baseline_set_source(&context, 1, 10, 20);
  rinzu_baseline_set_source(&context, 2, 30, 40);
  rinzu_baseline_set_source(&context, 1, 10, 20);
  rinzu_failure_fail(&context.pending, 1, context.source);
  same_source(context.pending.current.primary_source, (rinzu_source_position_t){1, 10, 20, 1});
  const rinzu_source_position_t caller = {1, 10, 20, 1}, callee = {2, 30, 40, 1};
  for (unsigned enabled = 0; enabled < 2; ++enabled) {
    rinzu_error_trace_stack_t *stack = &context.error_trace_stack;
    stack->initialized = 1;
    stack->enabled = enabled;
    rinzu_error_trace_stack_enter(stack, "caller", 6);
    packed_source(&context, caller);
    same_source(context.source, caller);
    rinzu_error_trace_stack_enter(stack, "callee", 6);
    packed_source(&context, callee);
    same_source(context.source, callee);
    if (enabled) {
      assert(stack->count == 2);
      same_source(stack->frames[0].source, caller);
      same_source(stack->frames[1].source, callee);
    }
    rinzu_error_trace_stack_leave(stack);
    /* Native reentry can also replace context.source. A repeated caller
       position must restore it rather than treating it as already current. */
    context.source = callee;
    packed_source(&context, caller);
    same_source(context.source, caller);
    if (enabled)
      same_source(stack->frames[0].source, caller);
    rinzu_error_trace_stack_leave(stack);
    rinzu_error_trace_stack_dispose(stack);
  }
}
/* Check the private generated-store region against the complete context,
   including source padding and the next field. Native reentry and nested
   managed calls must return to the same context-owned address. */
static void region_source(RinzuContext *context, uint64_t descriptor, uint64_t file, uint32_t start,
                          uint32_t end) {
  unsigned char before[sizeof(*context)];
  memcpy(before, context, sizeof before);
  unsigned char *region = (unsigned char *)(uintptr_t)(descriptor & ~UINT64_C(1));
  assert(region == (unsigned char *)&context->source);
  uint64_t span = (uint64_t)start | ((uint64_t)end << 32);
  uint32_t present = 1;
  memcpy(region, &file, sizeof file);
  memcpy(region + 8, &span, sizeof span);
  memcpy(region + 16, &present, sizeof present);
  if (descriptor & 1)
    rinzu_baseline_observe_source(context);
  same_source(context->source, (rinzu_source_position_t){file, start, end, 1});
  size_t source = offsetof(RinzuContext, source);
  for (size_t i = 0; i < sizeof(*context); ++i)
    if (i < source || i >= source + 20)
      assert(((unsigned char *)context)[i] == before[i]);
}
static void check_source_region(void) {
  for (unsigned trace = 0; trace < 2; ++trace) {
    RinzuContext context = {0}, other = {0};
    rinzu_owner_transition_t shared = {0};
    context.shared_transitions = &shared;
    context.polls = UINT64_C(0xfedcba9876543210);
    context.error_trace_stack.initialized = 1;
    context.error_trace_stack.enabled = trace;
    uint64_t descriptor = (uint64_t)rinzu_baseline_function_entry(&context, 0);
    assert((descriptor & 1) == trace);
    assert((descriptor & ~UINT64_C(1)) == (uintptr_t)&context.source);
    region_source(&context, descriptor, UINT32_MAX, UINT32_MAX - 1, UINT32_MAX);
    uint64_t nested = (uint64_t)rinzu_baseline_function_entry(&context, 0);
    assert(nested == descriptor);
    region_source(&context, nested, 0, 0, UINT32_MAX);
    if (trace) {
      assert(context.error_trace_stack.count == 2);
      same_source(context.error_trace_stack.frames[0].source,
                  (rinzu_source_position_t){UINT32_MAX, UINT32_MAX - 1, UINT32_MAX, 1});
      same_source(context.error_trace_stack.frames[1].source, context.source);
    }
    rinzu_error_trace_stack_leave(&context.error_trace_stack);
    rinzu_managed_leave(&context.roots);
    /* A callback can replace source; the caller must still store it again. */
    context.source = (rinzu_source_position_t){99, 11, 12, 1};
    region_source(&context, descriptor, 3, 17, 29);
    if (trace)
      same_source(context.error_trace_stack.frames[0].source, context.source);
    other.error_trace_stack.initialized = 1;
    other.error_trace_stack.enabled = trace;
    uint64_t separate = (uint64_t)rinzu_baseline_function_entry(&other, 0);
    assert((separate & ~UINT64_C(1)) != (descriptor & ~UINT64_C(1)));
    region_source(&other, separate, 7, 0, 0);
    same_source(context.source, (rinzu_source_position_t){3, 17, 29, 1});
    assert(context.polls == UINT64_C(0xfedcba9876543210));
    rinzu_managed_leave(&context.roots);
    rinzu_managed_leave(&other.roots);
    rinzu_error_trace_stack_leave(&context.error_trace_stack);
    rinzu_error_trace_stack_leave(&other.error_trace_stack);
    rinzu_error_trace_stack_dispose(&context.error_trace_stack);
    rinzu_error_trace_stack_dispose(&other.error_trace_stack);
    rinzu_managed_dispose(&context.roots);
    rinzu_managed_dispose(&other.roots);
  }
}
static void *wrong_owner(void *raw) {
  rinzu_baseline_publish_roots(raw, NULL, 0);
  return NULL;
}
static void check_empty_roots(void) {
  RinzuContext context = {0};
  rinzu_managed_roots_t parked = {0};
  rinzu_managed_root_set_t attached = {0};
  rinzu_managed_enter(&context.roots);
  uint64_t caller = rinzu_managed_string(&context.data, (const unsigned char *)"caller root", 11);
  rinzu_managed_publish(&context.roots, &caller, 1);
  rinzu_managed_enter(&parked);
  uint64_t other = rinzu_managed_string(&context.data, (const unsigned char *)"parked root", 11);
  rinzu_managed_publish(&parked, &other, 1);
  rinzu_managed_attach_roots(&context.data, &attached, &parked);
  rinzu_managed_enter(&context.roots);
  uint64_t temporary = rinzu_managed_string(&context.data, (const unsigned char *)"dead", 4);
  rinzu_baseline_publish_roots(&context, &temporary, 1);
  uint64_t publications = context.roots.publications;
  uint64_t collections = context.data.collections;
  rinzu_gc_allocated(&context.data.gc, RINZU_GC_MIN_BUDGET);
  rinzu_baseline_publish_roots(&context, NULL, 0);
  assert(context.data.collections == collections + 1 && context.data.objects == 2);
  assert(context.roots.publications == publications + 1 && !context.roots.current->count);
  assert(!rinzu_managed_lookup(&context.data, temporary));
  context.data.gc.stress = 1;
  for (unsigned i = 0; i < 4; ++i) {
    unsigned char *before = rinzu_managed_find(&context.data, caller)->bytes;
    rinzu_baseline_publish_roots(&context, NULL, 0);
    assert(rinzu_managed_find(&context.data, caller)->bytes != before);
    assert(rinzu_managed_find(&context.data, other)->length == 11);
  }
  context.data.gc.stress = 0;
  for (unsigned kind = 0; kind < 13; ++kind) {
    pid_t child = fork();
    assert(child >= 0);
    if (!child) {
      switch (kind) {
      case 0:
        context.data.live_views = 1;
        break;
      case 1:
        context.data.view_allocations = 1;
        break;
      case 2:
        (void)rinzu_baseline_prepare_roots(&context, 0);
        break;
      case 3:
        context.roots.current->count = context.roots.current->capacity + 1;
        break;
      case 4:
        context.roots.current = NULL;
        break;
      case 5: {
        pthread_t thread;
        assert(!pthread_create(&thread, NULL, wrong_owner, &context));
        pthread_join(thread, NULL);
        _Exit(1);
      }
      case 6:
        context.polls = UINT64_MAX;
        (void)rinzu_baseline_poll(&context, 0);
        _Exit(1);
      case 7:
        context.domain_polls = &collections;
        collections = UINT64_MAX;
        (void)rinzu_baseline_poll(&context, 0);
        _Exit(1);
      case 8:
        rinzu_baseline_set_source(&context, -1, 0, 0);
        _Exit(1);
      case 9:
        rinzu_baseline_set_source(&context, 0, -1, 1);
        _Exit(1);
      case 10:
        rinzu_baseline_set_source(&context, 0, 2, 1);
        _Exit(1);
      case 11:
        rinzu_baseline_set_source(&context, 0, 0, (int64_t)UINT32_MAX + 1);
        _Exit(1);
      case 12:
        rinzu_baseline_publish_roots(&context, NULL, -1);
        _Exit(1);
      }
      rinzu_baseline_publish_roots(&context, NULL, 0);
      _Exit(1);
    }
    int status;
    assert(waitpid(child, &status, 0) == child && WIFEXITED(status) && WEXITSTATUS(status) == 70);
  }
  rinzu_managed_leave(&context.roots);
  rinzu_managed_leave(&context.roots);
  rinzu_managed_leave(&parked);
  rinzu_managed_detach_roots(&context.data, &attached);
  rinzu_managed_collect(&context.data, &context.roots);
  assert(!context.data.objects);
  rinzu_managed_dispose(&context.roots);
  rinzu_managed_dispose(&parked);
  rinzu_managed_data_dispose(&context.data);
}
static void check_fused_publication_poll(void) {
  for (unsigned nonempty = 0; nonempty < 2; ++nonempty) {
    for (unsigned mode = 0; mode < 8; ++mode) {
      RinzuContext legacy = {0}, fused = {0};
      TestAsync asynchronous = {.explicit_shutdown = 1};
      TestWorker worker = {0};
      atomic_init(&worker.cancel_requested, 1);
      RinzuContext *contexts[] = {&legacy, &fused};
      uint64_t roots[2] = {0};
      for (unsigned index = 0; index < 2; ++index) {
        RinzuContext *context = contexts[index];
        rinzu_managed_enter(&context->roots);
        rinzu_baseline_set_source(context, 3, 7, 11);
        if (nonempty)
          roots[index] = rinzu_managed_string(&context->data, (const unsigned char *)"root", 4);
        context->data.gc.stress = 1;
        if (mode == 1 || mode == 3 || mode == 4)
          context->cancel_after = 1;
        if (mode == 2)
          context->shutdown_after = 1;
        if (mode == 3)
          context->cancel_masked = 1;
        if (mode == 4)
          context->callback_depth = 1;
        if (mode == 5) {
          context->pending.depth = 1;
          context->pending.current.primary = 1;
        }
        if (mode == 6)
          context->async = &asynchronous;
        if (mode == 7)
          context->worker = &worker;
      }
      if (nonempty) {
        *rinzu_baseline_prepare_roots(&legacy, 1) = roots[0];
        *rinzu_baseline_prepare_roots(&fused, 1) = roots[1];
        rinzu_baseline_commit_roots(&legacy, 1);
      } else {
        rinzu_baseline_publish_roots(&legacy, NULL, 0);
      }
      (void)rinzu_baseline_poll(&legacy, 7);
      int64_t expected = rinzu_baseline_has_failure(&legacy);
      int64_t status = nonempty ? rinzu_baseline_commit_roots_poll(&fused, 1)
                                : rinzu_baseline_publish_roots_poll(&fused, NULL, 0);
      assert(status == expected);
      assert(legacy.polls == 1 && fused.polls == 1);
      assert(legacy.shutdown_checks == fused.shutdown_checks &&
             legacy.cancel_checks == fused.cancel_checks);
      assert(legacy.cancel_requested == fused.cancel_requested &&
             legacy.shutdown_requested == fused.shutdown_requested);
      assert(legacy.pending.current.primary == fused.pending.current.primary);
      assert(legacy.roots.publications == fused.roots.publications);
      assert(legacy.data.collections == fused.data.collections);
      if (nonempty) {
        assert(rinzu_managed_find(&legacy.data, roots[0])->length == 4);
        assert(rinzu_managed_find(&fused.data, roots[1])->length == 4);
      }
      for (unsigned index = 0; index < 2; ++index) {
        rinzu_managed_leave(&contexts[index]->roots);
        rinzu_managed_dispose(&contexts[index]->roots);
        rinzu_managed_data_dispose(&contexts[index]->data);
      }
    }
  }
}
int main(void) {
  check_source();
  check_source_region();
  check_empty_roots();
  check_fused_publication_poll();
  puts("PASS poll reference, per-poll cancellation, source ranges, empty roots, Owner, views and "
       "GC");
  return 0;
}
