#include <assert.h>
#include "rinzu/native_input.h"

typedef struct {
  RinzuContext *context;
  uint64_t input_type, input;
  size_t result_type;
} BytesShareCall;
static void *bytes_share_foreign_thread(void *raw) {
  BytesShareCall *call = raw;
  (void)rinzu_baseline_data_operation(call->context, RINZU_DATA_BYTE_ARRAY,
                                      (int64_t)call->result_type, &call->input_type, &call->input,
                                      1);
  return NULL;
}
int main(int argc, char **argv) {
  assert(argc == 2);
  const char *mode = argv[1];
  RinzuContext context = {0}, foreign = {0};
  context.shared_data = &context.data;
  rinzu_managed_data_t *data = rinzu_context_data(&context);
  rinzu_managed_attach_roots(data, &context.domain_roots, &context.roots);
  rinzu_managed_enter(&context.roots);
  size_t dynamic = SIZE_MAX, scalar = SIZE_MAX, fixed[9];
  for (size_t index = 0; index < 9; ++index)
    fixed[index] = SIZE_MAX;
  size_t list = SIZE_MAX;
  for (size_t index = 0; index < data->schema->count; ++index) {
    const rinzu_value_type_t *type = &data->schema->types[index];
    if (type->kind == RINZU_VALUE_DYNAMIC_BYTES)
      dynamic = index;
    if (type->kind == RINZU_VALUE_BYTES && type->bytes_length < 9)
      fixed[type->bytes_length] = index;
    if (type->kind == RINZU_VALUE_SCALAR && type->scalar_kind == RINZU_SCALAR_I64)
      scalar = index;
    if (type->kind == RINZU_VALUE_LIST &&
        data->schema->types[type->element].kind == RINZU_VALUE_SCALAR &&
        data->schema->types[type->element].scalar_kind == RINZU_SCALAR_U8)
      list = index;
  }
  assert(dynamic != SIZE_MAX && scalar != SIZE_MAX && list != SIZE_MAX);
  assert(fixed[0] != SIZE_MAX && fixed[3] != SIZE_MAX && fixed[8] != SIZE_MAX);
  size_t length = !strcmp(mode, "empty") ? 0 : !strcmp(mode, "padding") ? 8 : 3;
  unsigned char native[8] = {0, 255, 128, 0, 0, 0, 0, 0};
  const unsigned char expected[8] = {0, 255, 128, 0, 0, 0, 0, 0};
  uint64_t input = 0;
  rinzu_managed_receive_bytes(data, length, length ? native : NULL, &input);
  memset(native, 90, sizeof(native)); /* The native snapshot must stay isolated. */
  BytesShareCall call = {&context, fixed[length], input, dynamic};
  if (!strcmp(mode, "length"))
    call.input_type = fixed[8];
  if (!strcmp(mode, "scalar"))
    call.input_type = scalar;
  if (!strcmp(mode, "kind"))
    rinzu_managed_lookup_owned(data, input)->kind = RINZU_MANAGED_STRING;
  if (!strcmp(mode, "result"))
    call.result_type = scalar;
  if (!strcmp(mode, "foreign")) {
    foreign.shared_data = &foreign.data;
    rinzu_managed_data_t *other = rinzu_context_data(&foreign);
    call.input = rinzu_managed_bytes_allocate_owned(other, length)->identity;
  }
  bool from_list = !strcmp(mode, "list") || !strcmp(mode, "list-result");
  if (from_list) {
    rinzu_managed_slot_t fields[] = {{0, 0}, {255, 0}, {128, 0}};
    assert(!rinzu_managed_composite_owned(data, list, -1, fields, 3, &call.input));
    call.input_type = list;
    if (!strcmp(mode, "list-result"))
      call.result_type = scalar;
  }
  if (!strcmp(mode, "owner")) {
    pthread_t thread;
    assert(!pthread_create(&thread, NULL, bytes_share_foreign_thread, &call));
    assert(!pthread_join(thread, NULL));
    return 3;
  }
  uint64_t shared = (uint64_t)rinzu_baseline_data_operation(
      &context, RINZU_DATA_BYTE_ARRAY, (int64_t)call.result_type, &call.input_type, &call.input, 1);
  assert(!strcmp(mode, "normal") || !strcmp(mode, "empty") || !strcmp(mode, "padding") ||
         !strcmp(mode, "list"));
  assert(from_list ? shared != call.input : shared == input);
  assert(context.allocations == (from_list ? 1u : 0u));
  uint64_t type = dynamic;
  uint64_t copied = (uint64_t)rinzu_baseline_data_operation(&context, RINZU_DATA_BYTES_COPY,
                                                            (int64_t)dynamic, &type, &shared, 1);
  assert(copied != shared);
  uint64_t roots[] = {shared, copied};
  rinzu_managed_publish(&context.roots, roots, 2);
  rinzu_managed_collect(data, &context.roots);
  assert(data->objects == 2);
  rinzu_managed_value_t *value = rinzu_managed_lookup_owned(data, shared);
  assert(value->length == length && (!length || !memcmp(value->bytes, expected, length)));
  rinzu_managed_publish(&context.roots, &copied, 1);
  rinzu_managed_collect(data, &context.roots);
  assert(data->objects == 1);
  value = rinzu_managed_lookup_owned(data, copied);
  assert(value->length == length && (!length || !memcmp(value->bytes, expected, length)));
  rinzu_managed_leave(&context.roots);
  rinzu_managed_collect(data, &context.roots);
  assert(!data->objects);
  rinzu_managed_detach_roots(data, &context.domain_roots);
  rinzu_managed_dispose(&context.roots);
  rinzu_managed_data_dispose(data);
  fprintf(stdout, "BYTES_SHARE=%s\n", mode);
  return 0;
}
