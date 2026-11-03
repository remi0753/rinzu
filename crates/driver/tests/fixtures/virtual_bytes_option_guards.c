/* Included after the actual generated runtime, with its main renamed. */
#include <assert.h>
typedef struct { RinzuContext *context; size_t option; uint64_t input; } C06OwnerCall;
static void *c06_foreign_owner(void *raw) {
  C06OwnerCall *call = raw;
  (void)rinzu_baseline_bytes_slice_payload(call->context, (int64_t)call->option,
                                          (int64_t)call->input, 1, 33);
  return NULL;
}
static size_t c06_option_type(rinzu_managed_data_t *data) {
  for (size_t i = 0; i < data->schema->count; ++i) {
    const rinzu_value_type_t *t = &data->schema->types[i];
    if (t->kind == RINZU_VALUE_ENUM && t->variant_count == 2 &&
        !t->variants[0].count && t->variants[1].count == 1 &&
        data->schema->types[t->variants[1].types[0]].kind == RINZU_VALUE_DYNAMIC_BYTES)
      return i;
  }
  abort();
}
int main(int argc, char **argv) {
  assert(argc == 2); const char *mode = argv[1];
  atomic_store(&rinzu_resource_enabled, 1);
  RinzuContext context = {0}, foreign = {0}; context.shared_data = &context.data;
  rinzu_managed_data_t *data = rinzu_context_data(&context);
  rinzu_managed_attach_roots(data, &context.domain_roots, &context.roots);
  rinzu_managed_enter(&context.roots);
  size_t option = c06_option_type(data), bytes = SIZE_MAX, scalar = SIZE_MAX;
  for (size_t i = 0; i < data->schema->count; ++i) {
    if (data->schema->types[i].kind == RINZU_VALUE_DYNAMIC_BYTES) bytes = i;
    if (data->schema->types[i].kind == RINZU_VALUE_SCALAR && data->schema->types[i].scalar_kind == RINZU_SCALAR_I64) scalar = i;
  }
  assert(bytes != SIZE_MAX && scalar != SIZE_MAX);
  rinzu_managed_value_t *input_value = rinzu_managed_bytes_allocate_owned(data, 64);
  uint64_t input = input_value->identity;
  for (size_t i = 0; i < 64; ++i) input_value->bytes[i] = (unsigned char)(i * 7);
  rinzu_managed_publish(&context.roots, &input, 1);
  int64_t result_type = (int64_t)option;
  if (!strcmp(mode, "owner")) {
    C06OwnerCall call = {&context, option, input};
    pthread_t thread; assert(!pthread_create(&thread, NULL, c06_foreign_owner, &call));
    pthread_join(thread, NULL); return 3;
  }
  if (!strcmp(mode, "foreign")) {
    foreign.shared_data = &foreign.data;
    input = rinzu_managed_bytes_allocate_owned(rinzu_context_data(&foreign), 1)->identity;
  }
  if (!strcmp(mode, "zero")) input = 0;
  if (!strcmp(mode, "kind")) input_value->kind = RINZU_MANAGED_STRING;
  if (!strcmp(mode, "overflow")) input_value->length = (size_t)INT64_MAX + 1;
  if (!strcmp(mode, "negative-type")) result_type = -1;
  if (!strcmp(mode, "large-type")) result_type = (int64_t)data->schema->count;
  if (!strcmp(mode, "scalar-type")) result_type = (int64_t)scalar;
  if (!strcmp(mode, "stale")) { rinzu_managed_publish(&context.roots, NULL, 0); rinzu_managed_collect(data, &context.roots); }
  if (!strcmp(mode, "borrow") || !strcmp(mode, "writing")) {
    if (!strcmp(mode, "borrow")) data->live_views = 1;
    else context.roots.current->writing = 1;
    /* These guards are retained at the compiler's unchanged safepoint, before
       the leaf entry; no native/callback is allowed in between by the plan. */
    rinzu_managed_safepoint(data, &context.roots); return 3;
  }
  if (strcmp(mode, "normal") && strncmp(mode, "oom", 3) && strncmp(mode, "legacy-oom", 10)) {
    (void)rinzu_baseline_bytes_slice_payload(&context, result_type, (int64_t)input, 1, 33); return 3;
  }
  if (!strncmp(mode, "oom", 3) || !strncmp(mode, "legacy-oom", 10)) {
    bool legacy = !strncmp(mode, "legacy-oom", 10);
    size_t point = (size_t)atoi(mode + (legacy ? 10 : 3)); assert(point >= 1 && point <= 3);
    c06_fail_at = c06_allocations + point;
    uint64_t result;
    if (legacy) {
      uint64_t types[] = {bytes, scalar, scalar}, bits[] = {input, 1, 33};
      result = (uint64_t)rinzu_baseline_data_operation(&context, RINZU_DATA_BYTES_SLICE, result_type, types, bits, 3);
    } else result = (uint64_t)rinzu_baseline_bytes_slice_payload(&context, result_type, (int64_t)input, 1, 33);
    assert(!legacy && point == 3 && result && context.allocations == 2);
    c06_fail_at = 0;
  }
  const int64_t ranges[][2] = {{0,64},{0,0},{1,33},{64,64},{-1,2},{2,1},{0,65},{INT64_MIN,0},{0,INT64_MAX}};
  for (size_t i = 0; i < sizeof ranges / sizeof *ranges; ++i) {
    uint64_t before = context.allocations; size_t objects = data->objects;
    uint64_t value = (uint64_t)rinzu_baseline_bytes_slice_payload(&context, (int64_t)option, (int64_t)input, ranges[i][0], ranges[i][1]);
    uint64_t charged = context.allocations - before;
    bool valid = ranges[i][0] >= 0 && ranges[i][0] <= ranges[i][1] && ranges[i][1] <= 64;
    bool copy = valid && ranges[i][1] - ranges[i][0] != 64;
    assert((value != 0) == valid && charged == 1u + (unsigned)copy);
    assert(data->objects == objects + (unsigned)copy);
    uint64_t types[] = {bytes, scalar, scalar}, bits[] = {input, (uint64_t)ranges[i][0], (uint64_t)ranges[i][1]};
    before = context.allocations;
    uint64_t legacy = (uint64_t)rinzu_baseline_data_operation(&context, RINZU_DATA_BYTES_SLICE, (int64_t)option, types, bits, 3);
    assert(context.allocations - before == charged);
    uint32_t tag; assert(!rinzu_managed_enum_variant(data, legacy, option, &tag) && tag == valid);
    uint64_t roots[] = {input, value, legacy}; rinzu_managed_publish(&context.roots, roots, 3);
    data->gc.stress = 1; rinzu_managed_collect(data, &context.roots);
    if (valid) {
      rinzu_managed_slot_t old; assert(!rinzu_managed_project(data, legacy, option, 1, 0, &old));
      rinzu_managed_value_t *a = rinzu_managed_find(data, value), *b = rinzu_managed_find(data, old.bits);
      assert(a->length == (size_t)(ranges[i][1] - ranges[i][0]) && a->length == b->length && !memcmp(a->bytes, b->bytes, a->length));
      assert(copy ? value != input : value == input);
    }
    rinzu_managed_publish(&context.roots, &input, 1); rinzu_managed_collect(data, &context.roots); assert(data->objects == 1);
  }
  rinzu_managed_leave(&context.roots); rinzu_managed_collect(data, &context.roots);
  assert(!data->objects && !data->heap_bytes);
  rinzu_managed_detach_roots(data, &context.domain_roots); rinzu_managed_dispose(&context.roots); rinzu_managed_data_dispose(data);
  assert(!rinzu_resource_counters[RINZU_RESOURCE_MANAGED].live);
  puts("C06:slice:physical-option-elided:logical-charges-equal:moving-roots:OK"); return 0;
}
