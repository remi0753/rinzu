#include "rinzu/callback_entry.h"
#include <assert.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
typedef struct {
  pthread_t origin;
  uint64_t identity;
  size_t calls;
  int nested;
  rinzu_callback_pointer_u64_entry_t callback;
} Context;
static const unsigned char bytes[4] = {0, 255, 7, 0};
static uint64_t handler(void *raw, const unsigned char *input) {
  Context *context = raw;
  assert(pthread_equal(context->origin, pthread_self()));
  assert(input && memcmp(input, bytes, sizeof(bytes)) == 0);
  context->calls++;
  if (context->nested == 1) {
    context->nested = 2;
    assert(context->callback.entry(input) == context->identity + 4);
    assert(!rinzu_callback_entry_settled(context->callback.control));
  } else if (context->nested == 2) {
    assert(!rinzu_callback_entry_close(context->callback.control));
    assert(rinzu_callback_entry_stats().in_flight == 2);
  }
  return context->identity + 4;
}
static void prepare_unopened(Context *context, uint64_t identity) {
  *context = (Context){.origin = pthread_self(), .identity = identity};
  context->callback = rinzu_callback_pointer_u64_prepare_deferred(handler, context);
  assert(!rinzu_callback_entry_settled(context->callback.control));
}
static void prepare(Context *context, uint64_t identity) {
  prepare_unopened(context, identity);
  rinzu_callback_entry_open(context->callback.control);
}
static void *wrong_thread(void *raw) {
  Context *context = raw;
  (void)context->callback.entry(bytes);
  abort();
}
static void *wrong_thread_open(void *raw) {
  Context *context = raw;
  rinzu_callback_entry_open(context->callback.control);
  abort();
}
enum { WORKERS = 8, PER_WORKER = 129, MAIN_COUNT = 1031 };
typedef struct {
  size_t index;
  rinzu_callback_pointer_u64_t *entries;
} Worker;
static void *worker(void *raw) {
  Worker *worker = raw;
  for (size_t index = 0; index < PER_WORKER; ++index) {
    Context context;
    prepare(&context, worker->index * PER_WORKER + index);
    worker->entries[index] = context.callback.entry;
    assert(context.callback.entry(bytes) == context.identity + 4);
    assert(rinzu_callback_entry_close(context.callback.control));
  }
  return NULL;
}
int main(int argc, char **argv) {
  const char *mode = argc > 1 ? argv[1] : "normal";
  if (!strcmp(mode, "oom-control"))
    rinzu_callback_test_fail_allocation_after(0);
  if (!strcmp(mode, "oom-block"))
    rinzu_callback_test_fail_allocation_after(1);
  if (!strcmp(mode, "resource"))
    rinzu_callback_test_fail_mapping(1);
  if (!strcmp(mode, "permission"))
    rinzu_callback_test_fail_mapping(2);
  if (!strcmp(mode, "merged-control"))
    rinzu_callback_test_merge_control_region();
  Context first;
  prepare_unopened(&first, 42);
  if (!strcmp(mode, "prepared")) {
    (void)first.callback.entry(bytes);
    abort();
  }
  if (!strcmp(mode, "prepared-close")) {
    assert(rinzu_callback_entry_close(first.callback.control));
    assert(rinzu_callback_entry_settled(first.callback.control));
    assert(rinzu_callback_entry_stats().live_connections == 0);
    (void)first.callback.entry(bytes);
    abort();
  }
  if (!strcmp(mode, "wrong-thread-open")) {
    pthread_t thread;
    assert(!pthread_create(&thread, NULL, wrong_thread_open, &first));
    assert(!pthread_join(thread, NULL));
    abort();
  }
  if (!strcmp(mode, "null-open")) {
    rinzu_callback_entry_open(NULL);
    abort();
  }
  rinzu_callback_entry_open(first.callback.control);
  if (!strcmp(mode, "double-open")) {
    rinzu_callback_entry_open(first.callback.control);
    abort();
  }
  if (!strcmp(mode, "reopen")) {
    assert(rinzu_callback_entry_close(first.callback.control));
    rinzu_callback_entry_open(first.callback.control);
    abort();
  }
  if (!strcmp(mode, "closed")) {
    assert(rinzu_callback_entry_close(first.callback.control));
    assert(rinzu_callback_entry_close(first.callback.control));
    first.calls = SIZE_MAX;
    (void)first.callback.entry(bytes);
    abort();
  }
  if (!strcmp(mode, "wrong-thread")) {
    pthread_t thread;
    assert(!pthread_create(&thread, NULL, wrong_thread, &first));
    assert(!pthread_join(thread, NULL));
    abort();
  }
  first.nested = 1;
  assert(first.callback.entry(bytes) == 46);
  assert(first.calls == 2 && rinzu_callback_entry_settled(first.callback.control));
  assert(rinzu_callback_entry_stats().live_connections == 0);
  Context *contexts = calloc(MAIN_COUNT, sizeof(*contexts));
  assert(contexts);
  for (size_t index = 0; index < MAIN_COUNT; ++index)
    prepare(&contexts[index], index);
  rinzu_callback_stats_t before = rinzu_callback_entry_stats();
  for (int repeat = 0; repeat < 3; ++repeat)
    for (size_t index = 0; index < MAIN_COUNT; ++index)
      assert(contexts[index].callback.entry(bytes) == index + 4);
  assert(rinzu_callback_entry_stats().entries == before.entries);
  for (size_t index = 0; index < MAIN_COUNT; ++index) {
    assert(rinzu_callback_entry_close(contexts[index].callback.control));
    for (size_t previous = 0; previous < index; ++previous)
      assert(contexts[index].callback.entry != contexts[previous].callback.entry);
  }
  free(contexts);
  pthread_t threads[WORKERS];
  Worker workers[WORKERS];
  rinzu_callback_pointer_u64_t entries[WORKERS * PER_WORKER];
  for (size_t index = 0; index < WORKERS; ++index) {
    workers[index] = (Worker){index, &entries[index * PER_WORKER]};
    assert(!pthread_create(&threads[index], NULL, worker, &workers[index]));
  }
  for (size_t index = 0; index < WORKERS; ++index)
    assert(!pthread_join(threads[index], NULL));
  for (size_t index = 0; index < WORKERS * PER_WORKER; ++index)
    for (size_t previous = 0; previous < index; ++previous)
      assert(entries[index] != entries[previous]);
  rinzu_callback_stats_t final = rinzu_callback_entry_stats();
  assert(final.entries == MAIN_COUNT + WORKERS * PER_WORKER + 1);
  assert(final.blocks == (final.entries + 511) / 512);
  assert(final.virtual_bytes == final.blocks * 32768);
  assert(final.live_connections == 0 && final.in_flight == 0);
  assert(final.closed_entries == final.entries && final.tombstone_bytes > 0 &&
         final.block_metadata_bytes > 0);
  assert(final.tombstone_bytes == final.control_bytes - final.blocks * 16384);
  assert(!final.live_diagnostics && !final.diagnostic_bytes);
  assert(final.invocations == MAIN_COUNT * 3 + WORKERS * PER_WORKER + 2);
  printf("PASS runtime callback entries=%zu blocks=%zu connections=%zu in_flight=%zu "
         "invocations=%zu\n",
         final.entries, final.blocks, final.live_connections, final.in_flight, final.invocations);
}
