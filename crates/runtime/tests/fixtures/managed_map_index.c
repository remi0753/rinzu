#include "rinzu/managed_composite.h"
#include <assert.h>

static const rinzu_value_type_t types[] = {
    {.kind = RINZU_VALUE_SCALAR, .scalar_kind = RINZU_SCALAR_I64},
    {.kind = RINZU_VALUE_STRING},
    {.kind = RINZU_VALUE_MAP, .key = 0, .value = 0},
    {.kind = RINZU_VALUE_MAP, .key = 1, .value = 0}};
static rinzu_managed_slot_t number(uint64_t bits) {
  return (rinzu_managed_slot_t){bits, 0};
}
static size_t find(rinzu_managed_data_t *data, uint64_t handle,
                   rinzu_managed_slot_t key, int collide) {
  rinzu_managed_value_t *map = rinzu_managed_find(data, handle);
  uint64_t hash = 0;
  size_t bucket, selected;
  assert(!rinzu_managed_value_hash(data, types[map->type].key, key, &hash));
  assert(!rinzu_managed_map_probe_owned(data, types[map->type].key, map->slots,
                                       map->map_index, key, collide ? 0 : hash,
                                       &bucket, &selected));
  return selected;
}
int main(void) {
  atomic_store(&rinzu_resource_enabled, 1);
  rinzu_value_schema_t schema = {0};
  assert(!rinzu_value_schema_init(&schema, types, 4));
  rinzu_managed_data_t data = {0}, other = {0};
  rinzu_managed_roots_t roots = {0};
  rinzu_managed_enter(&roots);
  assert(!rinzu_managed_configure_values(&data, &schema));
  assert(!rinzu_managed_configure_values(&other, &schema));
  uint64_t original = 0;
  const rinzu_managed_slot_t pairs[] = {{1,0},{10,0},{2,0},{20,0},{1,0},{30,0}};
  assert(!rinzu_managed_composite(&data, 2, -1, pairs, 6, &original));
  rinzu_managed_value_t *map = rinzu_managed_find(&data, original);
  assert(map->count == 4 && map->slots[1].bits == 30 && map->slots[2].bits == 2);
  /* Force equal full hashes, not only bucket collisions. Probe still compares
     keys and update preserves both entries, order and immutability. */
  free(map->map_index);
  map->map_index = rinzu_managed_map_index_new(3);
  /* Capacity stays identical (8); accounting is unchanged. */
  for (size_t slot = 0; slot < map->count; slot += 2)
    rinzu_managed_map_index_add(map->map_index, 0, slot);
  assert(find(&data, original, number(2), 1) == 2);
  assert(find(&data, original, number(3), 1) == SIZE_MAX);
  uint64_t updated = rinzu_managed_map_update_owned(&data, map, 2, number(2), number(40), 0, 0);
  uint64_t added = rinzu_managed_map_update_owned(
      &data, rinzu_managed_find(&data, updated), SIZE_MAX, number(3), number(50), 0, 0);
  uint64_t removed = rinzu_managed_map_update_owned(
      &data, rinzu_managed_find(&data, added), 2, number(2), number(0), 0, 1);
  const uint64_t live[] = {original, updated, added, removed};
  rinzu_managed_publish(&roots, live, 4);
  int previous_stress = data.gc.stress;
  data.gc.stress = 0;
  size_t original_bytes = rinzu_managed_find(&data, original)->gc_bytes;
  assert(original_bytes == sizeof(*map) + sizeof(pairs) +
                               rinzu_managed_map_index_bytes(map->map_index));
  for (int i = 0; i < 4; ++i)
    rinzu_managed_collect(&data, &roots);
  /* Deduplication leaves spare physical slots. Normal GC retains and charges
     that capacity; stress compacts it and releases exactly the removed bytes. */
  assert(rinzu_managed_find(&data, original)->gc_bytes == original_bytes);
  assert(rinzu_resource_counters[RINZU_RESOURCE_MANAGED].live ==
         data.heap_bytes + data.index_accounted_bytes);
  data.gc.stress = 1;
  rinzu_managed_collect(&data, &roots);
  assert(rinzu_managed_find(&data, original)->gc_bytes ==
         original_bytes - 2 * sizeof(rinzu_managed_slot_t));
  assert(rinzu_resource_counters[RINZU_RESOURCE_MANAGED].live ==
         data.heap_bytes + data.index_accounted_bytes);
  data.gc.stress = previous_stress;
  assert(find(&data, removed, number(3), 1) == 2);
  assert(find(&data, removed, number(2), 1) == SIZE_MAX);
  assert(rinzu_managed_find(&data, original)->slots[3].bits == 20);
  assert(rinzu_managed_find(&data, updated)->slots[3].bits == 40);
  assert(rinzu_managed_find(&data, added)->count == 6);
  uint64_t handle;
  assert(!rinzu_managed_composite(&data, 2, -1, NULL, 0, &handle));
  for (uint64_t i = 0; i < 1024; ++i) {
    uint64_t hash;
    assert(!rinzu_managed_value_hash(&data, 0, number(i), &hash));
    handle = rinzu_managed_map_update_owned(
        &data, rinzu_managed_find(&data, handle), SIZE_MAX, number(i), number(i + 100), hash, 0);
    rinzu_managed_publish(&roots, &handle, 1);
    rinzu_managed_safepoint(&data, &roots);
  }
  for (uint64_t i = 0; i < 1024; ++i) {
    size_t slot = find(&data, handle, number(i), 0);
    assert(slot == i * 2 && rinzu_managed_find(&data, handle)->slots[slot + 1].bits == i + 100);
  }
  map = rinzu_managed_find(&data, handle);
  for (size_t i = 0; i < 1024; ++i)
    handle = rinzu_managed_map_update_owned(
        &data, rinzu_managed_find(&data, handle), 0, number(0), number(0), 0, 1);
  assert(!rinzu_managed_find(&data, handle)->count);
  uint64_t a = rinzu_managed_string(&data, (const unsigned char *)"a\0b", 3);
  uint64_t b = rinzu_managed_string(&data, (const unsigned char *)"a\0b", 3);
  rinzu_managed_slot_t texts[] = {{a,1},{4,0},{b,1},{5,0}};
  assert(!rinzu_managed_composite(&data, 3, -1, texts, 4, &handle));
  assert(rinzu_managed_find(&data, handle)->count == 2);
  rinzu_managed_publish(&roots, &handle, 1);
  rinzu_managed_collect(&data, &roots);
  b = rinzu_managed_string(&data, (const unsigned char *)"a\0b", 3);
  assert(find(&data, handle, (rinzu_managed_slot_t){b,1}, 0) == 0);
  size_t before = data.objects;
  uint64_t untouched = 91;
  texts[0] = number(1);
  assert(rinzu_managed_composite(&data, 3, -1, texts, 2, &untouched) == RINZU_VALUE_TYPE);
  assert(untouched == 91 && data.objects == before);
  texts[0] = (rinzu_managed_slot_t){b,1};
  assert(rinzu_managed_composite(&other, 3, -1, texts, 2, &untouched) == RINZU_VALUE_TYPE);
  assert(!other.objects && untouched == 91);
  rinzu_managed_publish(&roots, NULL, 0);
  rinzu_managed_collect(&data, &roots);
  assert(!data.objects && !data.heap_bytes && !data.live_views && !data.view_allocations);
  rinzu_managed_leave(&roots);
  rinzu_managed_dispose(&roots);
  rinzu_managed_data_dispose(&data);
  rinzu_managed_data_dispose(&other);
  rinzu_value_schema_dispose(&schema);
  assert(!rinzu_resource_counters[RINZU_RESOURCE_MANAGED].live);
}
