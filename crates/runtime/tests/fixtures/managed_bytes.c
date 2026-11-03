#include <stdlib.h>
#include <string.h>
static size_t allocations;
static void *counted_calloc(size_t count, size_t size) {
  ++allocations;
  return calloc(count, size);
}
static inline void *counted_malloc(size_t size) {
  ++allocations;
  void *result = malloc(size);
  if (result) memset(result, 165, size);
  return result;
}
#define calloc counted_calloc
#define malloc counted_malloc
#include "rinzu/managed_composite.h"
#include <assert.h>
#include <stdio.h>
#include <sys/wait.h>

static void *wrong_owner(void *raw) {
  uint64_t result;
  rinzu_managed_bytes(raw, 0, NULL, 0, &result);
  return NULL;
}
int main(void) {
  atomic_store(&rinzu_resource_enabled, 1);
  const rinzu_value_type_t types[] = {{.kind = RINZU_VALUE_DYNAMIC_BYTES}};
  rinzu_value_schema_t schema = {0};
  assert(!rinzu_value_schema_init(&schema, types, 1));
  rinzu_managed_data_t data = {0}, foreign = {0};
  rinzu_managed_roots_t roots = {0};
  assert(!rinzu_managed_configure_values(&data, &schema));
  assert(!rinzu_managed_configure_values(&foreign, &schema));
  rinzu_managed_index_resize(&data, 128);
  rinzu_managed_enter(&roots);
  const size_t sizes[] = {0, 1, 15, 16, 256, 4096, 65536};
  uint64_t handles[sizeof(sizes) / sizeof(*sizes) + 1] = {0};
  for (size_t i = 0; i < sizeof(sizes) / sizeof(*sizes); ++i) {
    size_t before = allocations;
    rinzu_managed_value_t *value = rinzu_managed_bytes_allocate_owned(&data, sizes[i]);
    int inline_payload = data.inline_bytes_enabled && sizes[i] < sizeof(value->inline_bytes);
    assert(allocations == before + (inline_payload ? 1 : 2));
    assert((value->bytes == value->inline_bytes) == inline_payload);
    assert(value->gc_bytes == sizeof(*value) + (inline_payload ? 0 : sizes[i] + 1));
    for (size_t j = 0; j < sizes[i]; ++j)
      value->bytes[j] = (unsigned char)j;
    assert(!value->bytes[sizes[i]]);
    handles[i] = value->identity;
    assert(!rinzu_managed_slot_valid(&data, 0, (rinzu_managed_slot_t){handles[i], 1}));
    assert(rinzu_managed_slot_valid(&foreign, 0, (rinzu_managed_slot_t){handles[i], 1}) ==
           RINZU_VALUE_TYPE);
  }
  unsigned char native[] = {0, 255, 128, 42};
  uint64_t snapshot;
  assert(!rinzu_managed_bytes(&data, sizeof(native), native, sizeof(native), &snapshot));
  handles[sizeof(sizes) / sizeof(*sizes)] = snapshot;
  memset(native, 0, sizeof(native));
  assert(rinzu_managed_find(&data, snapshot)->bytes[1] == 255);
  size_t before = data.objects;
  assert(rinzu_managed_bytes(&data, 2, NULL, 2, &snapshot) == RINZU_VALUE_TYPE);
  assert(rinzu_managed_bytes(&data, 1, native, 2, &snapshot) == RINZU_VALUE_TYPE);
  assert(rinzu_managed_bytes(&data, 0, NULL, 0, NULL) == RINZU_VALUE_TYPE);
  assert(data.objects == before);
  rinzu_managed_publish(&roots, handles, sizeof(handles) / sizeof(*handles));
  data.gc.stress = 1; /* This fixture specifically verifies payload relocation. */
  for (unsigned repeat = 0; repeat < 3; ++repeat) {
    unsigned char *old = rinzu_managed_find(&data, handles[6])->bytes;
    unsigned char *short_old = rinzu_managed_find(&data, handles[1])->bytes;
    rinzu_managed_collect(&data, &roots);
    assert(rinzu_managed_find(&data, handles[6])->bytes != old);
    assert(rinzu_managed_find(&data, handles[1])->bytes != short_old);
    assert(!memcmp(rinzu_managed_find(&data, snapshot)->bytes, "\0\xff\x80\x2a", 4));
    for (size_t i = 0; i < sizeof(sizes) / sizeof(*sizes); ++i) {
      rinzu_managed_value_t *value = rinzu_managed_find(&data, handles[i]);
      assert(value->length == sizes[i] && !value->bytes[sizes[i]]);
      for (size_t j = 0; j < sizes[i]; ++j)
        assert(value->bytes[j] == (unsigned char)j);
    }
    assert(rinzu_resource_counters[RINZU_RESOURCE_MANAGED].live ==
           data.heap_bytes + data.index_accounted_bytes);
  }
  pid_t child = fork();
  assert(child >= 0);
  if (!child) {
    pthread_t thread;
    assert(!pthread_create(&thread, NULL, wrong_owner, &data));
    pthread_join(thread, NULL);
    _Exit(1);
  }
  int status;
  assert(waitpid(child, &status, 0) == child);
  assert(WIFEXITED(status) && WEXITSTATUS(status) == 70);
  rinzu_managed_leave(&roots);
  rinzu_managed_collect(&data, &roots);
  assert(!data.objects && !data.heap_bytes);
  rinzu_managed_dispose(&roots);
  rinzu_managed_data_dispose(&data);
  rinzu_managed_data_dispose(&foreign);
  rinzu_value_schema_dispose(&schema);
  assert(!rinzu_resource_counters[RINZU_RESOURCE_MANAGED].live);
  assert(!rinzu_resource_counters[RINZU_RESOURCE_MANAGED_OBJECTS].live);
  puts("PASS final byte payload, snapshots, binary/zero padding, moving GC, Owner and Domain");
}
