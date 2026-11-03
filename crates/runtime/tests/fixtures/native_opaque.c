#include <stdlib.h>
static _Thread_local size_t allocation_budget = (size_t)-1;
static void *controlled_calloc(size_t count, size_t size) {
  if (!allocation_budget)
    return NULL;
  if (allocation_budget != (size_t)-1)
    --allocation_budget;
  return calloc(count, size);
}
#define calloc controlled_calloc
#include "rinzu/native_opaque.h"
#include "rinzu/managed_composite.h"
#undef calloc
#include <assert.h>
#include <stdio.h>

static void run(void) {
  rinzu_managed_data_t data = {0};
  rinzu_managed_roots_t roots = {0};
  int native_object = 123;
  rinzu_native_opaque_reservation_t reservation = rinzu_managed_reserve_opaque(&data, 7);
  uint64_t handle = UINT64_MAX, provisional = reservation.identity;
  rinzu_managed_enter(&roots);
  rinzu_managed_publish(&roots, &provisional, 1);
  rinzu_managed_collect(&data, &roots);
  allocation_budget = 0;
  assert(!rinzu_managed_receive_opaque(&data, &reservation, &native_object, &handle));
  assert(rinzu_managed_opaque_pointer(&data, handle, 7) == &native_object);
  allocation_budget = (size_t)-1;
  assert(handle == provisional && reservation.identity == 0);
  rinzu_native_opaque_reservation_t duplicate = rinzu_managed_reserve_opaque(&data, 7);
  uint64_t other = 0;
  assert(!rinzu_managed_receive_opaque(&data, &duplicate, &native_object, &other));
  assert(other != handle);
  rinzu_native_opaque_reservation_t null = rinzu_managed_reserve_opaque(&data, 7);
  uint64_t unchanged = UINT64_MAX;
  allocation_budget = 0;
  assert(rinzu_managed_receive_opaque(&data, &null, NULL, &unchanged) == RINZU_NATIVE_NULL_OPAQUE);
  allocation_budget = (size_t)-1;
  assert(unchanged == UINT64_MAX && null.identity == 0 && data.objects == 2);
  rinzu_native_opaque_reservation_t cancelled = rinzu_managed_reserve_opaque(&data, 7);
  rinzu_managed_discard_opaque(&data, &cancelled);
  assert(!cancelled.identity);
  uint64_t live[] = {handle, other};
  rinzu_managed_publish(&roots, live, 2);
  for (int iteration = 0; iteration < 32; ++iteration) {
    rinzu_managed_collect(&data, &roots);
    assert(rinzu_managed_opaque_pointer(&data, handle, 7) == &native_object);
    assert(rinzu_managed_opaque_pointer(&data, other, 7) == &native_object);
  }
  // Explicit native state changes do not invalidate aliases or trigger probes.
  native_object = 456;
  assert(*(int *)rinzu_managed_opaque_pointer(&data, handle, 7) == 456);
  rinzu_managed_leave(&roots);
  rinzu_managed_collect(&data, &roots);
  assert(data.objects == 0 && native_object == 456);
  rinzu_managed_dispose(&roots);
  rinzu_managed_data_dispose(&data);

  size_t field = 0;
  rinzu_value_type_t types[] = {
      {.kind = RINZU_VALUE_OTHER},
      {.kind = RINZU_VALUE_RECORD, .identity = "Holder", .fields = &field, .field_count = 1}};
  rinzu_value_schema_t schema = {0};
  assert(!rinzu_value_schema_init(&schema, types, 2));
  assert(!rinzu_managed_configure_values(&data, &schema));
  reservation = rinzu_managed_reserve_opaque(&data, 0);
  assert(!rinzu_managed_receive_opaque(&data, &reservation, &native_object, &handle));
  rinzu_managed_slot_t slot = {handle, 1};
  uint64_t parent = 0;
  assert(!rinzu_managed_composite(&data, 1, -1, &slot, 1, &parent));
  rinzu_managed_enter(&roots);
  rinzu_managed_publish(&roots, &parent, 1);
  rinzu_managed_collect(&data, &roots);
  assert(data.objects == 2);
  rinzu_managed_slot_t projected = {0};
  assert(!rinzu_managed_project(&data, parent, 1, -1, 0, &projected));
  rinzu_managed_publish(&roots, &projected.bits, 1);
  rinzu_managed_collect(&data, &roots);
  assert(data.objects == 1);
  assert(rinzu_managed_opaque_pointer(&data, projected.bits, 0) == &native_object);
  rinzu_managed_leave(&roots);
  rinzu_managed_collect(&data, &roots);
  assert(data.objects == 0 && native_object == 456);
  rinzu_managed_dispose(&roots);
  rinzu_managed_data_dispose(&data);
  rinzu_value_schema_dispose(&schema);
}
static void *worker(void *unused) {
  (void)unused;
  run();
  return NULL;
}
struct foreign {
  rinzu_managed_data_t *data;
  uint64_t handle;
};
static void *foreign(void *raw) {
  struct foreign *value = raw;
  (void)rinzu_managed_opaque_pointer(value->data, value->handle, 7);
  return NULL;
}
int main(int argc, char **argv) {
  if (argc > 1) {
    rinzu_managed_data_t data = {0};
    if (!strcmp(argv[1], "oom"))
      allocation_budget = 0;
    rinzu_native_opaque_reservation_t reservation = rinzu_managed_reserve_opaque(&data, 7);
    uint64_t handle = reservation.identity;
    if (!strcmp(argv[1], "uninitialized"))
      (void)rinzu_managed_opaque_pointer(&data, handle, 7);
    assert(!rinzu_managed_receive_opaque(&data, &reservation, (void *)(uintptr_t)1, &handle));
    if (!strcmp(argv[1], "wrong-type"))
      (void)rinzu_managed_opaque_pointer(&data, handle, 8);
    if (!strcmp(argv[1], "wrong-domain")) {
      rinzu_managed_data_t other = {0};
      (void)rinzu_managed_opaque_pointer(&other, handle, 7);
    }
    if (!strcmp(argv[1], "foreign-thread")) {
      pthread_t thread;
      struct foreign value = {&data, handle};
      assert(!pthread_create(&thread, NULL, foreign, &value));
      assert(!pthread_join(thread, NULL));
    }
    rinzu_managed_data_dispose(&data);
    puts("NATIVE_ENTRY");
    return 1;
  }
  run();
  pthread_t threads[8];
  for (size_t index = 0; index < 8; ++index)
    assert(!pthread_create(&threads[index], NULL, worker, NULL));
  for (size_t index = 0; index < 8; ++index)
    assert(!pthread_join(threads[index], NULL));
  puts("PASS opaque Main/Worker preallocation allocation-free receive nonowning roots baseline");
}
