#include "rinzu/managed_composite.h"
#include <assert.h>
#include <sys/wait.h>
#include <unistd.h>

static void reject_unrooted_identity(rinzu_managed_data_t *data, rinzu_managed_roots_t *roots,
                                     uint64_t identity) {
  assert(!rinzu_gc_due(&data->gc));
  pid_t child = fork();
  assert(child >= 0);
  if (!child) {
    rinzu_managed_publish(roots, &identity, 1);
    rinzu_managed_safepoint(data, roots);
    _Exit(99);
  }
  int status;
  assert(waitpid(child, &status, 0) == child);
  assert(WIFEXITED(status) && WEXITSTATUS(status) == 70);
}

int main(void) {
  atomic_store(&rinzu_resource_enabled, 1);
  rinzu_managed_data_t data = {0}, other = {0};
  rinzu_managed_roots_t roots = {0};
  rinzu_managed_enter(&roots);
  enum { VALUE_COUNT = 2 * RINZU_MANAGED_LOOKUP_CACHE_CAPACITY + 2 };
  const size_t last = VALUE_COUNT - 1;
  uint64_t values[VALUE_COUNT];
  for (size_t i = 0; i < VALUE_COUNT; ++i) {
    unsigned char byte = (unsigned char)i;
    values[i] = rinzu_managed_string(&data, &byte, 1);
  }
  int enabled = !getenv("RINZU_MANAGED_LOOKUP_CACHE") ||
                strcmp(getenv("RINZU_MANAGED_LOOKUP_CACHE"), "0") != 0;
  assert(data.index_accounted_bytes ==
         data.index_capacity * sizeof(*data.index) +
             (enabled ? RINZU_MANAGED_LOOKUP_CACHE_CAPACITY * sizeof(*data.lookup_cache) : 0));
  for (size_t turn = 0; turn < 4; ++turn)
    for (size_t i = 0; i < VALUE_COUNT; ++i) {
      rinzu_managed_value_t *value = rinzu_managed_lookup(&data, values[i]);
      assert(value && value->bytes[0] == (unsigned char)i);
      assert(rinzu_managed_lookup(&data, values[i]) == value);
      if (enabled) {
        rinzu_managed_lookup_entry_t cached =
            data.lookup_cache[values[i] & (RINZU_MANAGED_LOOKUP_CACHE_CAPACITY - 1)];
        assert(cached.identity == values[i] && cached.value == value);
      } else
        assert(!data.lookup_cache);
      assert(!rinzu_managed_lookup(&other, values[i]));
    }
  /* Exercise a real low-bit collision at every configured cache capacity. */
  if (enabled) {
    uint64_t first = values[0], collision = values[RINZU_MANAGED_LOOKUP_CACHE_CAPACITY];
    assert((first & (RINZU_MANAGED_LOOKUP_CACHE_CAPACITY - 1)) ==
           (collision & (RINZU_MANAGED_LOOKUP_CACHE_CAPACITY - 1)));
    rinzu_managed_value_t *a = rinzu_managed_lookup(&data, first);
    rinzu_managed_value_t *b = rinzu_managed_lookup(&data, collision);
    assert(a && b && a != b);
    assert(rinzu_managed_lookup(&data, first) == a);
    assert(rinzu_managed_lookup(&data, collision) == b);
  }
  /* Rebuild only the bucket array. Stable metadata and weak tags need no new
     cache storage; its actual bytes are charged/released exactly once. */
  rinzu_managed_lookup_entry_t *cache = data.lookup_cache;
  size_t capacity = data.index_capacity;
  uint64_t allocated = rinzu_resource_counters[RINZU_RESOURCE_MANAGED].allocated;
  rinzu_managed_value_t *cached_value = rinzu_managed_lookup(&data, values[last]);
  rinzu_managed_index_resize(&data, capacity * 2);
  assert(data.lookup_cache == cache);
  assert(data.lookup_cache_accounted_bytes ==
         (enabled ? RINZU_MANAGED_LOOKUP_CACHE_CAPACITY * sizeof(*cache) : 0));
  assert(rinzu_resource_counters[RINZU_RESOURCE_MANAGED].allocated - allocated ==
         capacity * 2 * sizeof(*data.index));
  if (enabled) {
    rinzu_managed_lookup_entry_t entry =
        cache[values[last] & (RINZU_MANAGED_LOOKUP_CACHE_CAPACITY - 1)];
    assert(entry.identity == values[last] && entry.value == cached_value);
  }
  assert(rinzu_managed_lookup(&data, values[last]) == cached_value);
  rinzu_managed_index_resize(&data, capacity);
  assert(data.lookup_cache == cache);
  rinzu_managed_value_t *stable = rinzu_managed_lookup(&data, values[0]);
  int previous_stress = data.gc.stress;
  data.gc.stress = 1; /* Verify weak metadata also survives physical relocation. */
  const void *payload = stable->bytes;
  rinzu_managed_publish(&roots, values, 1);
  rinzu_managed_collect(&data, &roots);
  assert(rinzu_managed_lookup(&data, values[0]) == stable && stable->bytes != payload);
  data.gc.stress = previous_stress; /* The next guard specifically checks a non-due GC. */
  reject_unrooted_identity(&data, &roots, values[last]);
  for (size_t i = 1; i < VALUE_COUNT; ++i)
    assert(!rinzu_managed_lookup(&data, values[i]));
  /* A transaction can look up prepared children, then roll them back without
     collecting. Neither a freed hint nor a false hit may survive this path. */
  uint64_t prepared = rinzu_managed_string(&data, (const unsigned char *)"rolled back", 11);
  rinzu_managed_value_t *value = rinzu_managed_lookup(&data, prepared);
  assert(value == data.values);
  data.values = value->next;
  rinzu_managed_index_remove(&data, value);
  reject_unrooted_identity(&data, &roots, prepared);
  assert(!rinzu_managed_lookup(&data, prepared));
  rinzu_managed_value_release(&data, value);
  --data.objects;
  reject_unrooted_identity(&data, &roots, prepared);
  assert(!rinzu_managed_lookup(&data, prepared));
  assert(rinzu_managed_lookup(&data, values[0]) == stable);
  rinzu_managed_publish(&roots, NULL, 0);
  rinzu_managed_collect(&data, &roots);
  assert(!data.objects && !data.lookup_cache && !data.index);
  assert(!data.lookup_cache_accounted_bytes && !data.index_accounted_bytes);
  assert(!rinzu_managed_lookup(&data, values[0]));
  rinzu_managed_leave(&roots);
  rinzu_managed_dispose(&roots);
  rinzu_managed_data_dispose(&data);
  rinzu_managed_data_dispose(&other);
  assert(!rinzu_resource_counters[RINZU_RESOURCE_MANAGED].live);
}
