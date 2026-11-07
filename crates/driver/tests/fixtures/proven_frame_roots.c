#define main original_safepoints_main
#include "aot_safepoints.c"
#undef main
#include "rinzu/managed_composite.h"
#include "rinzu/native_opaque.h"
#include <string.h>

static int future_due_checks;
static int due(void *raw) {
  assert(raw == &future_due_checks);
  ++future_due_checks;
  return 1;
}
static const size_t cell_fields[] = {0};
static const rinzu_value_type_t cell_types[] = {
    {.kind = RINZU_VALUE_STRING},
    {.kind = RINZU_VALUE_RECORD, .fields = cell_fields, .field_count = 1},
    {.kind = RINZU_VALUE_OTHER},
};
static const rinzu_value_schema_t cell_schema = {.types = cell_types, .count = 3};

/* Callee and callback-like nested frames may collect. Fresh opaque NULL
   reservation retirement must not invalidate the caller's retained IDs. */
static void strong_caller_vectors(void) {
  RinzuContext context = {0};
  context.data.schema = &cell_schema;
  rinzu_managed_enter(&context.roots);
  uint64_t caller = rinzu_managed_string(&context.data, (const unsigned char *)"caller", 6);
  *rinzu_baseline_prepare_roots(&context, 1) = caller;
  assert(!rinzu_baseline_commit_roots_poll(&context, 1));
  for (unsigned round = 0; round < 64; ++round) {
    rinzu_managed_enter(&context.roots);
    uint64_t callee = rinzu_managed_string(&context.data, (const unsigned char *)"callee", 6);
    *rinzu_baseline_prepare_roots(&context, 1) = callee;
    assert(!rinzu_baseline_commit_roots_poll(&context, 1));
    rinzu_native_opaque_reservation_t reserved = rinzu_managed_reserve_opaque(&context.data, 2);
    uint64_t retired = reserved.identity, output = UINT64_MAX;
    assert(retired != caller && retired != callee);
    assert(rinzu_managed_receive_opaque(&context.data, &reserved, NULL, &output) ==
           RINZU_NATIVE_NULL_OPAQUE);
    assert(!reserved.identity && output == UINT64_MAX);
    assert(!rinzu_managed_lookup(&context.data, retired));
    rinzu_managed_collect(&context.data, &context.roots);
    assert(rinzu_managed_find(&context.data, caller)->length == 6);
    assert(rinzu_managed_find(&context.data, callee)->length == 6);
    rinzu_managed_leave(&context.roots);
    *rinzu_baseline_prepare_roots(&context, 1) = caller;
    assert(!rinzu_baseline_commit_proven_roots_poll(&context, 1));
  }
  rinzu_managed_publish(&context.roots, NULL, 0);
  rinzu_managed_collect(&context.data, &context.roots);
  assert(!context.data.objects && !rinzu_managed_lookup(&context.data, caller));
  rinzu_managed_leave(&context.roots);
  rinzu_managed_dispose(&context.roots);
  rinzu_managed_data_dispose(&context.data);
}

static void owned_vectors(void) {
  RinzuContext context = {0};
  context.data.schema = &cell_schema;
  rinzu_managed_enter(&context.roots);
  rinzu_baseline_set_source(&context, 9, 17, 29);
  context.data.future_due = due;
  context.data.future_context = &future_due_checks;
  for (unsigned round = 0; round < 128; ++round) {
    uint64_t text = rinzu_managed_string_literal(&context.data, (const unsigned char *)"owned", 5);
    rinzu_managed_slot_t input = {.bits = text, .reference = 1}, projected = {0};
    uint64_t cell = 0;
    assert(!rinzu_managed_constructor_charged(&context.data, 1, -1, &input, 1, &cell));
    assert(!rinzu_managed_project(&context.data, cell, 1, -1, 0, &projected));
    uint64_t *slots = rinzu_baseline_prepare_roots(&context, 4);
    slots[0] = text;
    slots[1] = cell;
    slots[2] = projected.bits;
    slots[3] = 0;
    assert(!rinzu_baseline_commit_proven_roots_poll(&context, 4));
    assert(context.polls == round + 1 && context.roots.current->count == 4);
    assert(rinzu_managed_find(&context.data, text)->length == 5);
    assert(context.data.collections == round + 1);
    rinzu_managed_slot_t after = {0};
    assert(!rinzu_managed_project(&context.data, cell, 1, -1, 0, &after));
    assert(after.bits == text && projected.bits == text);
  }
  assert(future_due_checks || context.data.gc.stress);
  context.data.future_due = NULL;
  uint64_t old = context.roots.current->roots[0];
  rinzu_managed_publish(&context.roots, NULL, 0);
  rinzu_managed_collect(&context.data, &context.roots);
  assert(!rinzu_managed_lookup(&context.data, old));
  uint64_t fresh = rinzu_managed_string_literal(&context.data, (const unsigned char *)"owned", 5);
  assert(fresh != old && rinzu_managed_find(&context.data, fresh)->length == 5);
  *rinzu_baseline_prepare_roots(&context, 1) = fresh;
  assert(!rinzu_baseline_commit_proven_roots_poll(&context, 1));
  rinzu_managed_leave(&context.roots);
  rinzu_managed_collect(&context.data, &context.roots);
  assert(!context.data.objects);
  rinzu_managed_dispose(&context.roots);
  rinzu_managed_data_dispose(&context.data);
}

static void poll_modes(void) {
  for (unsigned mode = 0; mode < 8; ++mode) {
    RinzuContext checked = {0}, proven = {0};
    TestAsync asynchronous = {.explicit_shutdown = 1};
    TestWorker worker = {0};
    atomic_init(&worker.cancel_requested, 1);
    RinzuContext *contexts[] = {&checked, &proven};
    for (unsigned side = 0; side < 2; ++side) {
      RinzuContext *context = contexts[side];
      rinzu_managed_enter(&context->roots);
      rinzu_baseline_set_source(context, 3, 7, 11);
      uint64_t text = rinzu_managed_string(&context->data, (const unsigned char *)"root", 4);
      *rinzu_baseline_prepare_roots(context, 1) = text;
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
    assert(rinzu_baseline_commit_roots_poll(&checked, 1) ==
           rinzu_baseline_commit_proven_roots_poll(&proven, 1));
    assert(checked.polls == 1 && proven.polls == 1);
    assert(checked.pending.current.primary == proven.pending.current.primary);
    same_source(checked.pending.current.primary_source, proven.pending.current.primary_source);
    assert(checked.cancel_checks == proven.cancel_checks &&
           checked.shutdown_checks == proven.shutdown_checks);
    assert(checked.cancel_requested == proven.cancel_requested &&
           checked.shutdown_requested == proven.shutdown_requested);
    assert(checked.roots.publications == proven.roots.publications &&
           checked.data.collections == proven.data.collections);
    for (unsigned side = 0; side < 2; ++side) {
      rinzu_managed_leave(&contexts[side]->roots);
      rinzu_managed_dispose(&contexts[side]->roots);
      rinzu_managed_data_dispose(&contexts[side]->data);
    }
  }
}
static void *foreign(void *raw) {
  (void)rinzu_baseline_commit_proven_roots_poll(raw, 1);
  return NULL;
}
static void guard(const char *name) {
  RinzuContext context = {0};
  context.data.schema = &cell_schema;
  rinzu_managed_enter(&context.roots);
  uint64_t root = rinzu_managed_string(&context.data, (const unsigned char *)"root", 4);
  if (!strcmp(name, "retired"))
    rinzu_managed_collect(&context.data, &context.roots);
  if (!strcmp(name, "domain") || !strcmp(name, "project-domain")) {
    rinzu_managed_data_t other = {0};
    root = rinzu_managed_string(&other, (const unsigned char *)"foreign", 7);
  }
  if (!strcmp(name, "project-domain")) {
    rinzu_managed_slot_t input = {.bits = root, .reference = 1};
    uint64_t output = 0;
    if (rinzu_managed_constructor_charged(&context.data, 1, -1, &input, 1, &output))
      rinzu_failure_fatal();
  }
  *rinzu_baseline_prepare_roots(&context, 1) = root;
  if (!strcmp(name, "views"))
    context.data.live_views = 1;
  if (!strcmp(name, "allocations"))
    context.data.view_allocations = 1;
  if (!strcmp(name, "writing"))
    context.roots.current->writing = 0;
  if (!strcmp(name, "foreign")) {
    pthread_t thread;
    assert(!pthread_create(&thread, NULL, foreign, &context));
    pthread_join(thread, NULL);
  } else if (!strcmp(name, "retired") || !strcmp(name, "domain"))
    (void)rinzu_baseline_commit_roots_poll(&context, 1);
  else
    (void)rinzu_baseline_commit_proven_roots_poll(&context, 1);
}
int main(int argc, char **argv) {
  if (argc > 1) {
    guard(argv[1]);
    return 1;
  }
  assert(!original_safepoints_main());
  owned_vectors();
  poll_modes();
  strong_caller_vectors();
  puts("PASS proven owned vectors, moving GC, stale cache, Future due, poll/failure and checked "
       "fallback");
  return 0;
}
