#include <stdlib.h>
#include <string.h>
static size_t allocation_count, fail_at;
static void *literal_calloc(size_t count, size_t size) {
  if (fail_at && ++allocation_count == fail_at)
    return NULL;
  return calloc(count, size);
}
#define calloc literal_calloc
#include "rinzu/managed_composite.h"
#include <assert.h>
#include <stdio.h>
#include <sys/wait.h>

static unsigned char binary[256];
static const unsigned char ascii[] = "literal\0tail";
static void *wrong_owner(void *raw) {
  rinzu_managed_bytes_literal(raw, ascii, sizeof(ascii) - 1);
  return NULL;
}
static void assert_bytes(rinzu_managed_data_t *data, uint64_t identity,
                         const unsigned char *bytes, size_t length) {
  rinzu_managed_value_t *value = rinzu_managed_find(data, identity);
  assert(value->kind == RINZU_MANAGED_BYTES && value->length == length);
  assert(!value->bytes[length]);
  assert(!length || !memcmp(value->bytes, bytes, length));
}
static void child_exit(pid_t child, int expected) {
  int status;
  assert(child > 0 && waitpid(child, &status, 0) == child);
  assert(WIFEXITED(status) && WEXITSTATUS(status) == expected);
}
int main(void) {
  atomic_store(&rinzu_resource_enabled, 1);
  for (size_t i = 0; i < sizeof(binary); ++i) binary[i] = (unsigned char)i;
  rinzu_managed_data_t data = {0}, foreign = {0};
  rinzu_managed_roots_t roots = {0};
  uint64_t handle = rinzu_managed_bytes_literal(&data, binary, sizeof(binary));
  uint64_t text = rinzu_managed_string_literal(&data, ascii, sizeof(ascii) - 1);
  uint64_t bytes = rinzu_managed_bytes_literal(&data, ascii, sizeof(ascii) - 1);
  assert(text != bytes && rinzu_managed_find(&data, text)->kind == RINZU_MANAGED_STRING);
  uint64_t empty = rinzu_managed_bytes_literal(&data, NULL, 0);
  size_t objects = data.objects, heap_bytes = data.heap_bytes;
  for (unsigned i = 0; i < 100; ++i) {
    assert(rinzu_managed_bytes_literal(&data, binary, sizeof(binary)) == handle);
    assert(rinzu_managed_string_literal(&data, ascii, sizeof(ascii) - 1) == text);
    assert(rinzu_managed_bytes_literal(&data, ascii, sizeof(ascii) - 1) == bytes);
    assert(rinzu_managed_bytes_literal(&data, NULL, 0) == empty);
  }
  assert(data.objects == objects && data.heap_bytes == heap_bytes);
  assert(rinzu_managed_bytes_literal(&data, ascii, 3) != bytes);
  assert_bytes(&data, bytes, ascii, sizeof(ascii) - 1);
  assert_bytes(&data, empty, NULL, 0);
  uint64_t other = rinzu_managed_bytes_literal(&foreign, binary, sizeof(binary));
  assert(other != handle && !rinzu_managed_lookup(&data, other));
  uint64_t dynamic_a, dynamic_b;
  unsigned char input[] = "literal\0tail";
  assert(!rinzu_managed_bytes(&data, sizeof(input) - 1, input, sizeof(input) - 1, &dynamic_a));
  input[0] = '!';
  assert(!rinzu_managed_bytes(&data, sizeof(input) - 1, input, sizeof(input) - 1, &dynamic_b));
  assert(dynamic_a != bytes && dynamic_b != dynamic_a);
  assert_bytes(&data, dynamic_a, ascii, sizeof(ascii) - 1);
  assert_bytes(&data, dynamic_b, input, sizeof(input) - 1);

  pid_t child = fork();
  assert(child >= 0);
  if (!child) {
    pthread_t thread;
    assert(!pthread_create(&thread, NULL, wrong_owner, &data));
    assert(!pthread_join(thread, NULL));
    _Exit(1);
  }
  child_exit(child, 70);
  rinzu_managed_enter(&roots);
  rinzu_managed_publish(&roots, &handle, 1);
  data.gc.stress = 1; /* This fixture specifically verifies payload relocation. */
  unsigned char *old = rinzu_managed_find(&data, handle)->bytes;
  rinzu_managed_collect(&data, &roots);
  assert(rinzu_managed_find(&data, handle)->bytes != old);
  assert(rinzu_managed_bytes_literal(&data, binary, sizeof(binary)) == handle);
  assert_bytes(&data, handle, binary, sizeof(binary));
  rinzu_managed_publish(&roots, NULL, 0);
  rinzu_managed_collect(&data, &roots);
  assert(!data.objects && !rinzu_managed_lookup(&data, handle));
  assert(data.heap_bytes == 2 * RINZU_MANAGED_LITERAL_CAPACITY * sizeof(*data.literals));
  uint64_t fresh = rinzu_managed_bytes_literal(&data, binary, sizeof(binary));
  assert(fresh != handle && fresh != other);
  assert_bytes(&data, fresh, binary, sizeof(binary));

  /* The physical removal used by rollback also invalidates a cached weak ID. */
  rinzu_managed_value_t *removed = data.values;
  assert(removed->identity == fresh);
  data.values = removed->next;
  rinzu_managed_index_remove(&data, removed);
  rinzu_managed_value_release(&data, removed);
  --data.objects;
  assert(rinzu_managed_bytes_literal(&data, binary, sizeof(binary)) != fresh);

  static unsigned char many[RINZU_MANAGED_LITERAL_CAPACITY * 2][4];
  for (size_t i = 0; i < sizeof(many) / sizeof(*many); ++i) {
    many[i][0] = (unsigned char)i;
    many[i][1] = (unsigned char)(i >> 8);
  }
  for (unsigned repeat = 0; repeat < 3; ++repeat) {
    for (size_t i = 0; i < sizeof(many) / sizeof(*many); ++i)
      assert_bytes(&data, rinzu_managed_bytes_literal(&data, many[i], sizeof(*many)),
                   many[i], sizeof(*many));
    rinzu_managed_collect(&data, &roots);
    assert(!data.objects);
  }
  assert(rinzu_resource_counters[RINZU_RESOURCE_MANAGED].live ==
         data.heap_bytes + data.index_accounted_bytes + foreign.heap_bytes + foreign.index_accounted_bytes);
  rinzu_managed_leave(&roots);
  rinzu_managed_dispose(&roots);
  rinzu_managed_data_dispose(&data);
  rinzu_managed_data_dispose(&foreign);
  assert(!rinzu_resource_counters[RINZU_RESOURCE_MANAGED].live);
  assert(!rinzu_resource_counters[RINZU_RESOURCE_MANAGED_OBJECTS].live);

  /* Cache, index, lookup cache, metadata and external payload all fail fatally. */
  const char *cache = getenv("RINZU_MANAGED_LOOKUP_CACHE");
  size_t allocations = !cache || strcmp(cache, "0") ? 5 : 4;
  for (size_t failure = 1; failure <= allocations; ++failure) {
    child = fork();
    assert(child >= 0);
    if (!child) {
      rinzu_managed_data_t oom = {0};
      fail_at = failure;
      allocation_count = 0;
      rinzu_managed_bytes_literal(&oom, binary, sizeof(binary));
      _Exit(1);
    }
    child_exit(child, 71);
  }
  puts("PASS byte literals: weak cache, kinds, snapshots, GC, rollback, collisions, Owner, Domain, OOM");
}
