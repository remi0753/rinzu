#include <assert.h>
#include <unistd.h>
static size_t length_type(rinzu_managed_data_t *data, unsigned kind, unsigned child_kind) {
  for (size_t i = 0; i < data->schema->count; ++i) {
    const rinzu_value_type_t *type = &data->schema->types[i];
    if (type->kind == kind &&
        (kind != RINZU_VALUE_LIST || data->schema->types[type->element].kind == child_kind)) return i;
  }
  abort();
}
static uint64_t length_list(rinzu_managed_data_t *data, size_t ty) {
  rinzu_managed_slot_t slots[] = {{11, 0}, {22, 0}};
  uint64_t result;
  assert(!rinzu_managed_composite(data, ty, -1, slots, 2, &result));
  return result;
}
struct length_owner_call { RinzuContext *context; uint64_t bits; size_t type; int kind; };
static void *length_foreign_owner(void *raw) {
  struct length_owner_call *c = raw;
  if (c->kind == 0) (void)rinzu_baseline_bytes_int_length(c->context, (int64_t)c->bits);
  else if (c->kind == 1) (void)rinzu_baseline_string_scalar_length(c->context, (int64_t)c->bits);
  else (void)rinzu_baseline_collection_length(c->context, (int64_t)c->bits, (int64_t)c->type);
  abort();
}
int main(int argc, char **argv) {
  assert(argc == 2); alarm(30);
  RinzuContext context = {0}; context.shared_data = &context.data;
  rinzu_managed_data_t *data = rinzu_context_data(&context);
  rinzu_managed_attach_roots(data, &context.domain_roots, &context.roots);
  rinzu_managed_enter(&context.roots);
  size_t ints = length_type(data, RINZU_VALUE_LIST, RINZU_VALUE_SCALAR);
  size_t strings = length_type(data, RINZU_VALUE_LIST, RINZU_VALUE_STRING);
  size_t map = length_type(data, RINZU_VALUE_MAP, 0);
  uint64_t bytes = rinzu_managed_bytes_literal(data, (const unsigned char *)"\0\xffz", 3);
  uint64_t text = rinzu_managed_string(data, (const unsigned char *)"A\xe6\x97\xa5\xf0\x9f\x98\x80\0", 9);
  uint64_t list = length_list(data, ints), empty;
  assert(!rinzu_managed_composite(data, map, -1, NULL, 0, &empty));
  const char *mode = argv[1];
  if (!strcmp(mode, "normal")) {
    uint64_t roots[] = {bytes, text, list, empty};
    rinzu_managed_publish(&context.roots, roots, 4);
    for (size_t moved = 0; moved < 3; ++moved) {
      size_t objects = data->objects, heap_bytes = data->heap_bytes;
      uint64_t allocations = context.allocations;
      for (size_t i = 0; i < 1000; ++i) {
        assert(rinzu_baseline_bytes_int_length(&context, (int64_t)bytes) == 3);
        assert(rinzu_baseline_string_scalar_length(&context, (int64_t)text) == 4);
        assert(rinzu_baseline_collection_length(&context, (int64_t)list, (int64_t)ints) == 2);
        assert(rinzu_baseline_collection_length(&context, (int64_t)empty, (int64_t)map) == 0);
      }
      assert(data->objects == objects && data->heap_bytes == heap_bytes && context.allocations == allocations);
      rinzu_managed_collect(data, &context.roots);
    }
    rinzu_managed_publish(&context.roots, NULL, 0);
    rinzu_managed_collect(data, &context.roots);
    assert(!rinzu_managed_lookup(data, bytes) && !rinzu_managed_lookup(data, list));
  } else if (!strcmp(mode, "byte-kind")) (void)rinzu_baseline_bytes_int_length(&context, (int64_t)text);
  else if (!strcmp(mode, "string-kind")) (void)rinzu_baseline_string_scalar_length(&context, (int64_t)bytes);
  else if (!strcmp(mode, "list-kind")) (void)rinzu_baseline_collection_length(&context, (int64_t)bytes, (int64_t)ints);
  else if (!strcmp(mode, "list-type")) (void)rinzu_baseline_collection_length(&context, (int64_t)list, (int64_t)strings);
  else if (!strcmp(mode, "map-type")) (void)rinzu_baseline_collection_length(&context, (int64_t)list, (int64_t)map);
  else if (!strcmp(mode, "negative-type")) (void)rinzu_baseline_collection_length(&context, (int64_t)list, -1);
  else if (!strcmp(mode, "past-type")) (void)rinzu_baseline_collection_length(&context, (int64_t)list, (int64_t)data->schema->count);
  else if (!strcmp(mode, "variant")) {
    rinzu_managed_find(data, list)->has_variant = 1;
    (void)rinzu_baseline_collection_length(&context, (int64_t)list, (int64_t)ints);
  } else if (!strcmp(mode, "odd-map")) {
    rinzu_managed_find(data, empty)->count = 1;
    (void)rinzu_baseline_collection_length(&context, (int64_t)empty, (int64_t)map);
  } else if (!strcmp(mode, "byte-overflow")) {
    rinzu_managed_find(data, bytes)->length = SIZE_MAX;
    (void)rinzu_baseline_bytes_int_length(&context, (int64_t)bytes);
  } else if (!strcmp(mode, "string-overflow")) {
    rinzu_managed_find(data, text)->scalar_length = SIZE_MAX;
    (void)rinzu_baseline_string_scalar_length(&context, (int64_t)text);
  } else if (!strcmp(mode, "list-overflow")) {
    rinzu_managed_find(data, list)->count = SIZE_MAX;
    (void)rinzu_baseline_collection_length(&context, (int64_t)list, (int64_t)ints);
  } else if (!strncmp(mode, "foreign-", 8)) {
    rinzu_managed_data_t other = {0}; assert(!rinzu_managed_configure_values(&other, data->schema));
    if (!strcmp(mode, "foreign-byte")) {
      uint64_t foreign = rinzu_managed_bytes_literal(&other, (const unsigned char *)"x", 1);
      (void)rinzu_baseline_bytes_int_length(&context, (int64_t)foreign);
    } else if (!strcmp(mode, "foreign-string")) {
      uint64_t foreign = rinzu_managed_string(&other, (const unsigned char *)"x", 1);
      (void)rinzu_baseline_string_scalar_length(&context, (int64_t)foreign);
    } else {
      assert(!strcmp(mode, "foreign-list"));
      (void)rinzu_baseline_collection_length(&context, (int64_t)length_list(&other, ints), (int64_t)ints);
    }
  } else if (!strcmp(mode, "stale")) {
    rinzu_managed_collect(data, &context.roots);
    (void)rinzu_baseline_collection_length(&context, (int64_t)list, (int64_t)ints);
  } else if (!strcmp(mode, "zero")) (void)rinzu_baseline_bytes_int_length(&context, 0);
  else {
    assert(!strcmp(mode, "owner-byte") || !strcmp(mode, "owner-string") || !strcmp(mode, "owner-list"));
    int kind = !strcmp(mode, "owner-byte") ? 0 : !strcmp(mode, "owner-string") ? 1 : 2;
    struct length_owner_call call = {&context, kind == 0 ? bytes : kind == 1 ? text : list, ints, kind};
    pthread_t thread; assert(!pthread_create(&thread, NULL, length_foreign_owner, &call));
    assert(!pthread_join(thread, NULL));
  }
  if (strcmp(mode, "normal")) abort();
  rinzu_managed_leave(&context.roots); rinzu_managed_detach_roots(data, &context.domain_roots);
  rinzu_managed_dispose(&context.roots); rinzu_managed_data_dispose(data);
  return 0;
}
