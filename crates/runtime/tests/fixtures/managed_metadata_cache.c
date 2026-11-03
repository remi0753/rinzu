/* Bounded metadata reuse: stale IDs, ownership, zeroing, accounting and pacing. */
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>
static size_t metadata_size;
static _Atomic size_t metadata_allocations, watched_frees;
static unsigned fail_allocations;
static void *watched;
static void *counted_calloc(size_t n, size_t size) {
  if (fail_allocations) return NULL;
  if (metadata_size && n && size == metadata_size / n && n * size == metadata_size)
    ++metadata_allocations;
  return calloc(n, size);
}
static void counted_free(void *pointer) {
  if (pointer && pointer == watched) ++watched_frees;
  free(pointer);
}
#define calloc counted_calloc
#define free counted_free
#include "rinzu/managed_composite.h"
#include "rinzu/native_opaque.h"
#undef calloc
#undef free

static void retire_head(rinzu_managed_data_t *data) {
  rinzu_managed_value_t *value = data->values;
  assert(value && data->objects);
  data->values = value->next;
  rinzu_managed_index_remove(data, value);
  rinzu_managed_value_release(data, value);
  --data->objects;
}
static void check_zeroed(const rinzu_managed_value_t *v) {
  rinzu_managed_value_t expected;
  memset(&expected, 0, sizeof expected);
  expected.identity = v->identity;
  expected.kind = v->kind;
  expected.accounted_bytes = v->accounted_bytes;
  expected.gc_bytes = v->gc_bytes;
  expected.next = v->next;
  expected.index_next = v->index_next;
  assert(!memcmp(v, &expected, sizeof expected));
}
static void *foreign_owner(void *raw) {
  (void)rinzu_managed_new(raw, RINZU_MANAGED_BYTES);
  return NULL;
}
static void *own_worker(void *unused) {
  (void)unused;
  rinzu_managed_data_t data = {0};
  uint64_t previous = 0;
  for (unsigned i=0; i<40; ++i) {
    uint64_t current = rinzu_managed_string(&data, (const unsigned char *)"worker", 6);
    assert(current != previous && !rinzu_managed_lookup(&data, previous));
    retire_head(&data);
    previous = current;
  }
  assert(!data.objects);
  rinzu_managed_data_dispose(&data);
  return NULL;
}
static void check_gc_sequence(void) {
  rinzu_managed_data_t data = {0};
  rinzu_managed_roots_t roots = {0};
  rinzu_managed_enter(&roots);
  for (unsigned i=0; i<10000; ++i) {
    unsigned char byte=(unsigned char)i;
    uint64_t value=rinzu_managed_string(&data,&byte,1);
    rinzu_managed_publish(&roots,&value,1);
    rinzu_managed_safepoint(&data,&roots);
  }
  printf("PACE:%llu:%zu:%zu:%zu:%zu:%zu\n", (unsigned long long)data.collections,
         data.gc.allocated,data.gc.budget,data.objects,data.peak_objects,data.heap_bytes);
  rinzu_managed_leave(&roots);
  rinzu_managed_collect(&data,&roots);
  rinzu_managed_dispose(&roots);
  rinzu_managed_data_dispose(&data);
}
static void check_error_reuse(rinzu_managed_data_t *data) {
  rinzu_source_position_t source={.file=42,.start_byte=3,.end_byte=8,.present=1};
  rinzu_error_trace_frame_t frame={"guard",5,source};
  rinzu_error_trace_t *trace=rinzu_error_trace_capture(&frame,1);
  rinzu_diagnostic_node_t cleanup={.cause=12,.source=source,.trace=trace};
  rinzu_failure_t failure={.primary=5,.primary_source=source,.primary_trace=trace,
                           .head=&cleanup,.tail=&cleanup};
  uint64_t identity=rinzu_managed_error(data,&failure);
  rinzu_managed_value_t *error=rinzu_managed_find(data,identity);
  assert(error == data->values && atomic_load(&trace->references) == 3);
  retire_head(data);
  assert(atomic_load(&trace->references) == 1);
  rinzu_managed_value_t *fresh=rinzu_managed_new(data,RINZU_MANAGED_COMPOSITE);
#if RINZU_MANAGED_METADATA_CACHE_CAPACITY
  assert(fresh == error);
#endif
  assert(fresh->identity != identity && !rinzu_managed_lookup(data,identity));
  check_zeroed(fresh);
  rinzu_error_trace_release(trace);
}
int main(int argc, char **argv) {
  assert(argc == 2);
  metadata_size = sizeof(rinzu_managed_value_t);
  unsigned accounting = strcmp(argv[1], "normal-off") != 0;
  atomic_store(&rinzu_resource_enabled, accounting);
  rinzu_managed_data_t data = {0}, other = {0};
  rinzu_managed_roots_t roots = {0};
  /* Keep an index alive so a metadata hit has no unrelated index allocation. */
  (void)rinzu_managed_string(&data, (const unsigned char *)"anchor", 6);
  uint64_t stale = rinzu_managed_string(&data, (const unsigned char *)"secret payload", 14);
  rinzu_managed_value_t *old = rinzu_managed_find(&data, stale);
  watched = old;
  old->marked = 17;
  old->type = 123;
  old->has_variant = 1;
  old->variant = 42;
  old->mark_next = data.values->next;
  size_t allocations = metadata_allocations;
  size_t before_budget = data.gc.allocated;
  retire_head(&data);
  assert(!rinzu_managed_lookup(&data, stale));
#if RINZU_MANAGED_METADATA_CACHE_CAPACITY
  assert(!watched_frees && data.metadata_cache_count == 1);
  assert(rinzu_resource_counters[RINZU_RESOURCE_RUNTIME_CONTROL].live ==
         (accounting ? metadata_size : 0));
  fail_allocations = 1;
#else
  assert(watched_frees == 1 && !data.metadata_cache_count);
#endif
  rinzu_managed_value_t *fresh = rinzu_managed_new(&data, RINZU_MANAGED_COMPOSITE);
  fail_allocations = 0;
  check_zeroed(fresh);
  assert(fresh->identity != stale && !rinzu_managed_lookup(&data, stale));
  assert(data.gc.allocated == before_budget + metadata_size);
#if RINZU_MANAGED_METADATA_CACHE_CAPACITY
  assert(fresh == old && metadata_allocations == allocations);
  assert(!data.metadata_cache_count);
  assert(!rinzu_resource_counters[RINZU_RESOURCE_RUNTIME_CONTROL].live);
#else
  assert(metadata_allocations == allocations + 1);
#endif
  if (!strcmp(argv[1], "oom")) {
    fail_allocations = 1;
    (void)rinzu_managed_new(&data, RINZU_MANAGED_COMPOSITE);
    assert(!"cold metadata allocation failure was ignored");
  }
  if (!strcmp(argv[1], "identity")) {
    retire_head(&data);
    atomic_store(&rinzu_managed_next_identity, INT64_MAX);
    (void)rinzu_managed_new(&data, RINZU_MANAGED_COMPOSITE);
    assert(!"identity exhaustion was bypassed by reuse");
  }
  if (!strcmp(argv[1], "stale")) {
    rinzu_managed_enter(&roots);
    rinzu_managed_publish(&roots, &stale, 1);
    rinzu_managed_safepoint(&data, &roots);
    assert(!"stale identity survived reuse");
  }
  if (!strcmp(argv[1], "owner")) {
    pthread_t thread;
    assert(!pthread_create(&thread, NULL, foreign_owner, &data));
    assert(!pthread_join(thread, NULL));
    assert(!"foreign physical Owner accepted");
  }
  uint64_t foreign = rinzu_managed_string(&other, (const unsigned char *)"domain", 6);
  assert(!rinzu_managed_lookup(&data, foreign));
  if (!strcmp(argv[1], "domain")) {
    rinzu_managed_enter(&roots);
    rinzu_managed_publish(&roots, &foreign, 1);
    rinzu_managed_safepoint(&data, &roots);
    assert(!"foreign Domain identity accepted");
  }
  assert(!strcmp(argv[1], "normal") || !strcmp(argv[1], "normal-off"));
  check_error_reuse(&data);
  /* Discard must remain allocation-free, including a populated cache. */
  rinzu_native_opaque_reservation_t reservation = rinzu_managed_reserve_opaque(&data, 0);
  uint64_t opaque = reservation.identity;
  fail_allocations = 1;
  rinzu_managed_discard_opaque(&data, &reservation);
  fail_allocations = 0;
  assert(!reservation.identity && !rinzu_managed_lookup(&data, opaque));
  while (data.values) retire_head(&data);
  assert(!data.heap_bytes && !data.objects);
  rinzu_managed_index_trim(&data);
  /* A collection neither traces idle metadata nor changes mutator pacing. */
  rinzu_managed_collect(&data, &roots);
  assert(!data.objects && !data.heap_bytes && !data.gc.allocated);
  for (size_t i=0; i<RINZU_MANAGED_METADATA_CACHE_CAPACITY+3; ++i)
    (void)rinzu_managed_new(&data, RINZU_MANAGED_COMPOSITE);
  while (data.values) retire_head(&data);
  assert(data.metadata_cache_count == RINZU_MANAGED_METADATA_CACHE_CAPACITY);
  assert(rinzu_resource_counters[RINZU_RESOURCE_RUNTIME_CONTROL].live ==
         (accounting ? data.metadata_cache_count * metadata_size : 0));
  watched = NULL;
  rinzu_managed_data_dispose(&data);
  rinzu_managed_data_dispose(&other);
  assert(!data.metadata_cache && !data.metadata_cache_count);
  assert(!rinzu_resource_counters[RINZU_RESOURCE_MANAGED].live);
  assert(!rinzu_resource_counters[RINZU_RESOURCE_MANAGED_OBJECTS].live);
  assert(!rinzu_resource_counters[RINZU_RESOURCE_RUNTIME_CONTROL].live);
  check_gc_sequence();
  pthread_t threads[4];
  for (unsigned i=0; i<4; ++i)
    assert(!pthread_create(&threads[i], NULL, own_worker, NULL));
  for (unsigned i=0; i<4; ++i)
    assert(!pthread_join(threads[i], NULL));
  assert(!rinzu_resource_counters[RINZU_RESOURCE_RUNTIME_CONTROL].live);
  printf("PASS metadata=%zu capacity=%u accounting=%u reuse, zeroing, weak IDs, pacing, discard and disposal\n",
         metadata_size, (unsigned)RINZU_MANAGED_METADATA_CACHE_CAPACITY, accounting);
}
