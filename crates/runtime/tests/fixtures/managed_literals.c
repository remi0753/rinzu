#include "rinzu/managed_data.h"
#include <assert.h>
#include <stdio.h>
#include <sys/wait.h>

static const unsigned char text[] = "日本語🙂\0literal";
static void *wrong_owner(void *raw) {
  rinzu_managed_string_literal(raw, text, sizeof(text) - 1);
  return NULL;
}

int main(void) {
  atomic_store(&rinzu_resource_enabled, 1);
  rinzu_managed_data_t data = {0}, foreign = {0};
  rinzu_managed_roots_t roots = {0};
  uint64_t handle = rinzu_managed_string_literal(&data, text, sizeof(text) - 1);
  size_t objects = data.objects, bytes = data.heap_bytes;
  for (int i = 0; i < 100; i++)
    assert(rinzu_managed_string_literal(&data, text, sizeof(text) - 1) == handle);
  assert(data.objects == objects && data.heap_bytes == bytes);
  uint64_t other = rinzu_managed_string_literal(&foreign, text, sizeof(text) - 1);
  assert(other != handle && !rinzu_managed_lookup(&data, other));
  assert(rinzu_managed_string_value(&data, handle)->scalar_length == 12);
  pid_t child = fork();
  assert(child >= 0);
  if (!child) {
    pthread_t thread;
    assert(!pthread_create(&thread, NULL, wrong_owner, &data));
    pthread_join(thread, NULL);
    _Exit(1);
  }
  int status = 0;
  assert(waitpid(child, &status, 0) == child);
  assert(WIFEXITED(status) && WEXITSTATUS(status) == 70);

  rinzu_managed_enter(&roots);
  rinzu_managed_publish(&roots, &handle, 1);
  data.gc.stress = 1; /* This fixture specifically verifies payload relocation. */
  unsigned char *old = rinzu_managed_string_value(&data, handle)->bytes;
  rinzu_managed_collect(&data, &roots);
  assert(rinzu_managed_string_literal(&data, text, sizeof(text) - 1) == handle);
  assert(rinzu_managed_string_value(&data, handle)->bytes != old);
  assert(!memcmp(rinzu_managed_string_value(&data, handle)->bytes, text, sizeof(text)));
  rinzu_managed_publish(&roots, NULL, 0);
  rinzu_managed_collect(&data, &roots);
  assert(!data.objects && !rinzu_managed_lookup(&data, handle));
  assert(data.heap_bytes == RINZU_MANAGED_LITERAL_CAPACITY * sizeof(*data.literals));
  uint64_t fresh = rinzu_managed_string_literal(&data, text, sizeof(text) - 1);
  assert(fresh != handle && fresh != other);
  assert(!memcmp(rinzu_managed_string_value(&data, fresh)->bytes, text, sizeof(text)));
  assert(rinzu_managed_string_literal(&data, NULL, 0) ==
         rinzu_managed_string_literal(&data, NULL, 0));

  /* More distinct immutable program-lifetime sources than cache entries force
     replacement. Hash collisions cannot return a different literal's value. */
  static unsigned char many[RINZU_MANAGED_LITERAL_CAPACITY * 2][4];
  for (size_t i = 0; i < sizeof(many) / sizeof(*many); i++) {
    many[i][0] = (unsigned char)('A' + i / 26);
    many[i][1] = (unsigned char)('a' + i % 26);
  }
  for (int repeat = 0; repeat < 3; repeat++) {
    for (size_t i = 0; i < sizeof(many) / sizeof(*many); i++) {
      uint64_t value = rinzu_managed_string_literal(&data, many[i], 2);
      assert(!memcmp(rinzu_managed_string_value(&data, value)->bytes, many[i], 3));
    }
    rinzu_managed_collect(&data, &roots);
    assert(!data.objects);
  }
  assert(rinzu_resource_counters[RINZU_RESOURCE_MANAGED].live ==
         data.heap_bytes + data.index_accounted_bytes + foreign.heap_bytes +
             foreign.index_accounted_bytes);
  rinzu_managed_leave(&roots);
  rinzu_managed_dispose(&roots);
  rinzu_managed_data_dispose(&data);
  rinzu_managed_data_dispose(&foreign);
  assert(!rinzu_resource_counters[RINZU_RESOURCE_MANAGED].live);
  assert(!rinzu_resource_counters[RINZU_RESOURCE_MANAGED_OBJECTS].live);
  puts("PASS weak literal cache, moving GC, stale handles, collisions, Owner and Domain isolation");
}
