#include <assert.h>
#include <pthread.h>
#include <stdio.h>
#include <string.h>
#include "rinzu/managed_errors.h"

static const size_t byte_child[] = {7};
static const rinzu_key_variant_t options[] = {{0, NULL}, {1, byte_child}};
static const rinzu_value_type_t types[] = {
    {.kind = RINZU_VALUE_SCALAR, .scalar_kind = RINZU_SCALAR_I64},
    {.kind = RINZU_VALUE_STRING},
    {.kind = RINZU_VALUE_LIST, .element = 0},
    {.kind = RINZU_VALUE_LIST, .element = 1},
    {.kind = RINZU_VALUE_STATE, .identity = "EmptyState"},
    {.kind = RINZU_VALUE_RECORD, .identity = "EmptyRecord"},
    {.kind = RINZU_VALUE_ENUM, .identity = "ByteOption", .variant_count = 2, .variants = options},
    {.kind = RINZU_VALUE_SCALAR, .scalar_kind = RINZU_SCALAR_U8},
    {.kind = RINZU_VALUE_ERROR},
    {.kind = RINZU_VALUE_MAP, .key = 0, .value = 1}};
static uint64_t make(rinzu_managed_data_t *d, size_t type, int mode) {
  uint64_t id = 0;
  int status = mode == 0   ? rinzu_managed_composite(d, type, -1, NULL, 0, &id)
               : mode == 1 ? rinzu_managed_composite_fresh(d, type, -1, NULL, 0, &id)
                           : rinzu_managed_constructor_charged(d, type, -1, NULL, 0, &id);
  assert(!status);
  return id;
}
static void inspect(rinzu_managed_data_t *d, uint64_t id, size_t type) {
  rinzu_managed_value_t *v = rinzu_managed_find(d, id);
  assert(v->kind == RINZU_MANAGED_COMPOSITE && v->type == type && !v->has_variant && !v->count &&
         !v->slots);
}
static void *foreign(void *raw) {
  (void)make(raw, 2, 2);
  return NULL;
}
int main(int argc, char **argv) {
  atomic_store(&rinzu_resource_enabled, 1);
  rinzu_value_schema_t schema = {0};
  assert(!rinzu_value_schema_init(&schema, types, sizeof types / sizeof *types));
  rinzu_managed_data_t d = {0}, other = {0};
  assert(!rinzu_managed_configure_values(&d, &schema));
  assert(!rinzu_managed_configure_values(&other, &schema));
  uint64_t first = make(&d, 2, 2), again = make(&d, 2, 2);
  if (argc == 2) {
    assert(!strcmp(argv[1], "owner"));
    pthread_t t;
    assert(!pthread_create(&t, NULL, foreign, &d));
    assert(!pthread_join(t, NULL));
    return 3;
  }
  assert((first == again) == (RINZU_MANAGED_SMALL_ENUM_MODE != 0));
  inspect(&d, again, 2);
  assert(make(&other, 2, 2) != first && make(&d, 3, 2) != first);
  /* No sharing in physical-object-delta/generic callers, even when warmed. */
  assert(make(&d, 2, 0) != make(&d, 2, 0));
  assert(make(&d, 2, 1) != make(&d, 2, 1));
  for (size_t type = 4; type <= 5; ++type)
    assert(make(&d, type, 2) != make(&d, type, 2));
  assert(make(&d, 9, 2) != make(&d, 9, 2));
  uint64_t output = UINT64_C(0x12345678);
  size_t objects = d.objects;
  assert(rinzu_managed_constructor_charged(&d, 2, 0, NULL, 0, &output) == RINZU_VALUE_TYPE);
  rinzu_managed_slot_t bad = {first, 1};
  assert(rinzu_managed_constructor_charged(&d, 2, -1, &bad, 1, &output) == RINZU_VALUE_TYPE);
  assert(rinzu_managed_constructor_charged(&d, 8, -1, NULL, 0, &output) == RINZU_VALUE_TYPE);
  assert(output == UINT64_C(0x12345678) && d.objects == objects);
  /* Existing U8 canonical entries may collide but never alias a List. */
  for (unsigned i = 0; i < 256; ++i) {
    rinzu_managed_slot_t byte = {i, 0};
    uint64_t opt = 0;
    assert(!rinzu_managed_composite(&d, 6, 1, &byte, 1, &opt));
    rinzu_managed_value_t *v = rinzu_managed_find(&d, opt);
    assert(v->type == 6 && v->slots[0].bits == i);
    inspect(&d, make(&d, 2, 2), 2);
  }
  rinzu_managed_roots_t roots = {0};
  rinzu_managed_enter(&roots);
  uint64_t empty = make(&d, 2, 2);
  rinzu_managed_slot_t value = {42, 0};
  uint64_t populated =
      rinzu_managed_list_update_owned(&d, rinzu_managed_find(&d, empty), SIZE_MAX, value, 0);
  inspect(&d, empty, 2);
  assert(rinzu_managed_find(&d, populated)->slots[0].bits == 42);
  uint64_t live[] = {empty, populated};
  rinzu_managed_publish(&roots, live, 2);
  rinzu_managed_collect(&d, &roots);
  inspect(&d, live[0], 2);
  assert(rinzu_managed_find(&d, live[1])->slots[0].bits == 42);
  uint64_t held = make(&d, 2, 2);
  assert((held == live[0]) == (RINZU_MANAGED_SMALL_ENUM_MODE != 0));
  rinzu_managed_publish(&roots, NULL, 0);
  rinzu_managed_collect(&d, &roots);
  assert(!d.objects && !rinzu_managed_lookup(&d, empty));
  /* Recycled metadata has a fresh full identity; stale weak hints miss. */
  uint64_t recycled = rinzu_managed_string(&d, (const unsigned char *)"reuse", 5);
  uint64_t fresh = make(&d, 2, 2);
  assert(fresh != empty && fresh != recycled);
  inspect(&d, fresh, 2);
#if RINZU_MANAGED_SMALL_ENUM_MODE
  assert(d.small_enum_accounted_bytes == 1536);
#endif
  rinzu_managed_leave(&roots);
  rinzu_managed_collect(&d, &roots);
  assert(!d.objects);
  rinzu_managed_dispose(&roots);
  rinzu_managed_data_dispose(&d);
  rinzu_managed_data_dispose(&other);
  rinzu_value_schema_dispose(&schema);
  assert(!rinzu_resource_counters[RINZU_RESOURCE_MANAGED].live);
  assert(!rinzu_resource_counters[RINZU_RESOURCE_MANAGED_OBJECTS].live);
  assert(!rinzu_resource_counters[RINZU_RESOURCE_RUNTIME_CONTROL].live);
  puts("PASS emptyList: explicit-only weak typed Owner aliases GC staleID freshState/Error Map "
       "quota-callers resources0");
}
