#include <assert.h>

/* Run the actual generated operation, including its closed-schema boundary. */
int main(int argc, char **argv) {
  assert(argc == 2);
  const char *mode = argv[1];
  RinzuContext context = {0}, foreign = {0};
  context.shared_data = &context.data;
  rinzu_managed_data_t *data = rinzu_context_data(&context);
  rinzu_managed_attach_roots(data, &context.domain_roots, &context.roots);
  rinzu_managed_enter(&context.roots);
  size_t bytes_type = SIZE_MAX, list_type = SIZE_MAX, scalar_type = SIZE_MAX;
  for (size_t index = 0; index < data->schema->count; ++index) {
    const rinzu_value_type_t *type = &data->schema->types[index];
    if (type->kind == RINZU_VALUE_DYNAMIC_BYTES) bytes_type = index;
    if (type->kind == RINZU_VALUE_SCALAR && type->scalar_kind == RINZU_SCALAR_I64)
      scalar_type = index;
  }
  for (size_t index = 0; index < data->schema->count; ++index) {
    const rinzu_value_type_t *type = &data->schema->types[index];
    if (type->kind == RINZU_VALUE_LIST && type->element == bytes_type) list_type = index;
  }
  assert(bytes_type != SIZE_MAX && list_type != SIZE_MAX && scalar_type != SIZE_MAX);
  rinzu_managed_value_t *empty = rinzu_managed_bytes_allocate_owned(data, 0);
  rinzu_managed_value_t *first = rinzu_managed_bytes_allocate_owned(data, 3);
  rinzu_managed_value_t *last = rinzu_managed_bytes_allocate_owned(data, 2);
  memcpy(first->bytes, (const unsigned char[]){0, 255, 128}, 3);
  memcpy(last->bytes, (const unsigned char[]){254, 0}, 2);
  rinzu_managed_slot_t fields[] = {{empty->identity, 1}, {first->identity, 1},
                                  {empty->identity, 1}, {last->identity, 1}};
  size_t count = !strcmp(mode, "empty") ? 0 : !strcmp(mode, "single") ? 3 : 4;
  rinzu_managed_slot_t many[1024];
  unsigned char expected_bytes[1024] = {0, 255, 128, 254, 0};
  size_t expected = !strcmp(mode, "empty") ? 0 : !strcmp(mode, "single") ? 3 : 5;
  bool boundary = false, all_empty = !strcmp(mode, "empty129");
  bool one = !strcmp(mode, "single129");
  if (!strncmp(mode, "parts", 5)) { count = (size_t)strtoul(mode + 5, NULL, 10); boundary = true; }
  if (!strncmp(mode, "zeros", 5)) { count = (size_t)strtoul(mode + 5, NULL, 10); boundary = true; }
  if (all_empty || one) { count = 129; boundary = true; }
  if (!strcmp(mode, "resize64")) { count = 64; boundary = true; }
  if (!strcmp(mode, "resize256")) { count = 256; boundary = true; }
  if (!strncmp(mode, "late-", 5)) { count = 257; boundary = true; }
  rinzu_managed_slot_t *slots = fields;
  uint64_t shared = first->identity;
  if (!strncmp(mode, "one", 3)) {
    count = 1;
    bool blank = !strcmp(mode, "one-empty");
    slots = fields + (blank ? 0 : 1);
    expected = blank ? 0 : 3;
  }
  if (boundary) {
    assert(count >= 2 && count <= 1024);
    expected = 0;
    for (size_t index = 0; index < count; ++index) {
      bool blank = all_empty || (one && index != count / 2) ||
                   (!strncmp(mode, "zeros", 5) && !(index % 2));
      rinzu_managed_value_t *part = empty;
      if (!blank) {
        part = rinzu_managed_bytes_allocate_owned(data, 1);
        part->bytes[0] = (unsigned char)index;
        expected_bytes[expected++] = (unsigned char)index;
        shared = part->identity;
      }
      many[index] = (rinzu_managed_slot_t){part->identity, 1};
    }
    slots = many;
  }
  uint64_t list = 0;
  assert(!rinzu_managed_composite_owned(data, list_type, -1, slots, count, &list));
  rinzu_managed_value_t *sequence = rinzu_managed_lookup_owned(data, list);
  if (!strcmp(mode, "scalar")) sequence->slots[3].reference = 0;
  if (!strcmp(mode, "one-scalar")) sequence->slots[0].reference = 0;
  if (!strcmp(mode, "one-kind")) first->kind = RINZU_MANAGED_STRING;
  if (!strcmp(mode, "late-scalar")) sequence->slots[count - 1].reference = 0;
  if (!strcmp(mode, "trailing")) last->kind = RINZU_MANAGED_STRING;
  if (!strcmp(mode, "overflow")) first->length = INT64_MAX;
  if (!strcmp(mode, "late-kind"))
    rinzu_managed_lookup_owned(data, sequence->slots[count - 1].bits)->kind = RINZU_MANAGED_STRING;
  if (!strcmp(mode, "late-overflow"))
    rinzu_managed_lookup_owned(data, sequence->slots[count - 1].bits)->length = INT64_MAX;
  if (!strcmp(mode, "foreign") || !strcmp(mode, "late-foreign")) {
    foreign.shared_data = &foreign.data;
    rinzu_managed_data_t *other = rinzu_context_data(&foreign);
    rinzu_managed_attach_roots(other, &foreign.domain_roots, &foreign.roots);
    rinzu_managed_enter(&foreign.roots);
    sequence->slots[count - 1].bits = rinzu_managed_bytes_allocate_owned(other, 1)->identity;
  }
  size_t capacity = data->index_capacity;
  bool resize = !strcmp(mode, "resize64") || !strcmp(mode, "resize256");
  if (resize)
    while (data->objects < capacity / 2)
      (void)rinzu_managed_bytes_allocate_owned(data, 0);
  uint64_t types[] = {list_type}, bits[] = {list};
  size_t result_type = !strcmp(mode, "result") || !strcmp(mode, "one-result") ? scalar_type : bytes_type;
  uint64_t result = (uint64_t)rinzu_baseline_data_operation(
      &context, RINZU_DATA_BYTES_CONCAT_ALL, (int64_t)result_type, types, bits, 1);
  assert(!strcmp(mode, "normal") || !strcmp(mode, "empty") || !strcmp(mode, "single") ||
         !strcmp(mode, "one") || !strcmp(mode, "one-empty") ||
         (boundary && strncmp(mode, "late-", 5)));
  if (!strcmp(mode, "single") || !strcmp(mode, "one")) {
    assert(result == first->identity && !context.allocations);
  } else if (all_empty || one || !strcmp(mode, "one-empty")) {
    assert(result == (one ? shared : empty->identity) && !context.allocations);
  } else assert(context.allocations == 1);
  if (resize) assert(data->index_capacity == capacity * 2);
  rinzu_managed_publish(&context.roots, &result, 1);
  rinzu_managed_collect(data, &context.roots);
  assert(data->objects == 1);
  rinzu_managed_value_t *value = rinzu_managed_lookup_owned(data, result);
  assert(value->length == expected);
  if (expected) assert(!memcmp(value->bytes, expected_bytes, expected));
  rinzu_managed_leave(&context.roots);
  rinzu_managed_collect(data, &context.roots);
  assert(!data->objects);
  rinzu_managed_detach_roots(data, &context.domain_roots);
  rinzu_managed_dispose(&context.roots);
  rinzu_managed_data_dispose(data);
  fprintf(stdout, "BYTES_CONCAT_ALL=%s\n", mode);
  return 0;
}
