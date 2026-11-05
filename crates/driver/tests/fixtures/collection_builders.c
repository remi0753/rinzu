#ifdef RINZU_COLLECTION_PROBE
#include <assert.h>

static uint64_t collection(RinzuContext *context, size_t type, const rinzu_managed_slot_t *slots,
                           size_t count) {
  uint64_t output = 0;
  assert(!rinzu_managed_composite(rinzu_context_data(context), type, -1, slots, count, &output));
  return output;
}

static uint64_t update(RinzuContext *context, int operation, size_t type, size_t integer,
                       uint64_t input, size_t key, size_t value, size_t arity) {
  const uint64_t types[] = {type, integer, integer};
  const uint64_t bits[] = {input, key, value};
  return (uint64_t)rinzu_baseline_data_operation(context, operation, type, types, bits,
                                                 (int64_t)arity);
}

static void expect_overflow(RinzuContext *context, size_t type, size_t current, size_t added) {
  assert(context->pending.current.primary == 15);
  const rinzu_error_reference_t *error = context->pending.current.primary_error;
  assert(error);
  rinzu_managed_data_t *data = rinzu_context_data(context);
  const rinzu_managed_value_t *object = rinzu_managed_find(data, error->handle);
  assert(object->type == type);
  const rinzu_managed_value_t *payload = rinzu_managed_find(data, object->slots[4].bits);
  assert(payload->count == 2 && payload->slots[0].bits == added &&
         payload->slots[1].bits == current);
  rinzu_failure_release(context->pending.current);
  context->pending.current = (rinzu_failure_t){0};
}

int main(void) {
  RinzuContext context = {0};
  rinzu_managed_data_t *data = rinzu_context_data(&context);
  size_t ints = SIZE_MAX, map = SIZE_MAX, parts = SIZE_MAX, integer = SIZE_MAX;
  for (size_t i = 0; i < data->schema->count; i++) {
    const rinzu_value_type_t *type = &data->schema->types[i];
    if (type->kind == RINZU_VALUE_SCALAR && type->scalar_kind == RINZU_SCALAR_I64)
      integer = i;
  }
  for (size_t i = 0; i < data->schema->count; i++) {
    const rinzu_value_type_t *type = &data->schema->types[i];
    if (type->kind == RINZU_VALUE_LIST && type->element == integer)
      ints = i;
    if (type->kind == RINZU_VALUE_MAP && type->key == integer && type->value == integer)
      map = i;
  }
  for (size_t i = 0; i < data->schema->count; i++) {
    const rinzu_value_type_t *type = &data->schema->types[i];
    if (type->kind == RINZU_VALUE_LIST && type->element == ints)
      parts = i;
  }
  assert(ints != SIZE_MAX && map != SIZE_MAX && parts != SIZE_MAX);
  const size_t sizes[] = {1024, 2048};
  for (size_t run = 0; run < 2; run++) {
    size_t n = sizes[run];
    for (int is_map = 0; is_map < 2; is_map++) {
      size_t type = is_map ? map : ints;
      int operation = is_map ? RINZU_DATA_MAP_BUFFER_INSERT : RINZU_DATA_LIST_BUFFER_APPEND;
      size_t stride = is_map ? 2 : 1;
      uint64_t empty = collection(&context, type, NULL, 0), output = empty;
      rinzu_managed_index_resize(data, 4096);
      copy_bytes = allocations = hashes = 0;
      uint64_t logical = context.allocations;
      measuring = 1;
      for (size_t i = 0; i < n; i++)
        output = update(&context, operation, type, integer, output, i, i, stride + 1);
      measuring = 0;
      size_t growth = run + 9;
      assert(context.allocations - logical == n);
      assert(allocations == 1 + growth * stride);
      assert(copy_bytes == (n - 4) * stride * sizeof(rinzu_managed_slot_t));
      assert(hashes == (is_map ? n : 0));
      const rinzu_managed_map_index_t *index = rinzu_managed_find(data, output)->map_index;
      if (is_map) {
        size_t before = copy_bytes, allocated = allocations;
        measuring = 1;
        for (size_t i = 0; i < n; i++)
          assert(update(&context, operation, type, integer, output, 3, i, 3) == output);
        measuring = 0;
        assert(copy_bytes == before && allocations == allocated && hashes == n * 2);
        assert(rinzu_managed_find(data, output)->map_index == index);
      }
      const uint64_t types[] = {type};
      measuring = 1;
      assert((uint64_t)rinzu_baseline_data_operation(&context, RINZU_DATA_COLLECTION_FINISH, type,
                                                     types, &output, 1) == output);
      measuring = 0;
      assert(allocations == 1 + growth * stride);
      assert(copy_bytes == (n - 4) * stride * sizeof(rinzu_managed_slot_t));
      const rinzu_managed_value_t *published = rinzu_managed_find(data, output);
      assert(!published->collection_building && published->count == n * stride);
      assert(rinzu_managed_find(data, empty)->count == 0);
      for (size_t i = 0; i < n; i++)
        assert(published->slots[i * stride].bits == i);
      if (is_map)
        assert(published->slots[7].bits == n - 1);
    }
  }

  assert(rinzu_collection_length_fits(INT64_MAX - 1, 1));
  assert(rinzu_collection_length_fits(INT64_MAX, 0));
  assert(!rinzu_collection_length_fits(INT64_MAX, 1));
  uint64_t big = collection(&context, ints, NULL, 0);
  rinzu_managed_value_t *large = rinzu_managed_find(data, big);
  large->count = INT64_MAX;
  assert(!update(&context, RINZU_DATA_LIST_APPEND, ints, integer, big, 1, 0, 2));
  expect_overflow(&context, rinzu_list_length_overflow, INT64_MAX, 1);
  large->count = INT64_MAX - 1;

  uint64_t two = collection(&context, ints, NULL, 0);
  rinzu_managed_find(data, two)->count = 2;
  rinzu_managed_slot_t slices[] = {{big, 1}, {two, 1}};
  uint64_t nested = collection(&context, parts, slices, 2);
  const uint64_t types[] = {parts};
  assert(!rinzu_baseline_data_operation(&context, RINZU_DATA_LIST_CONCAT_ALL, ints, types, &nested,
                                        1));
  expect_overflow(&context, rinzu_list_length_overflow, INT64_MAX - 1, 2);
  large->count = 0;
  rinzu_managed_find(data, two)->count = 0;

  uint64_t full = collection(&context, map, NULL, 0);
  rinzu_managed_find(data, full)->count = (size_t)INT64_MAX * 2;
  assert(!update(&context, RINZU_DATA_MAP_INSERT, map, integer, full, 1, 2, 3));
  expect_overflow(&context, rinzu_map_length_overflow, INT64_MAX, 1);
  rinzu_managed_find(data, full)->count = 0;

  rinzu_managed_collect(data, &context.roots);
  assert(!data->objects && !data->heap_bytes);
  rinzu_managed_data_dispose(data);
  return 0;
}
#endif
