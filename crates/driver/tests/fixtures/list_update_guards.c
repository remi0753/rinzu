#include <assert.h>
#include <unistd.h>
static size_t list_type(rinzu_managed_data_t *data, unsigned element_kind) {
  for (size_t i = 0; i < data->schema->count; ++i) {
    const rinzu_value_type_t *type = &data->schema->types[i];
    if (type->kind == RINZU_VALUE_LIST && data->schema->types[type->element].kind == element_kind &&
        (element_kind != RINZU_VALUE_SCALAR ||
         data->schema->types[type->element].scalar_kind == RINZU_SCALAR_I64)) return i;
  }
  abort();
}
static size_t option_type(rinzu_managed_data_t *data, size_t payload) {
  for (size_t i = 0; i < data->schema->count; ++i) {
    const rinzu_value_type_t *type = &data->schema->types[i];
    if (type->kind == RINZU_VALUE_ENUM && type->variant_count == 2 &&
        !type->variants[0].count && type->variants[1].count == 1 &&
        type->variants[1].types[0] == payload) return i;
  }
  abort();
}
static uint64_t make_list(rinzu_managed_data_t *data, size_t type, size_t count,
                           rinzu_managed_slot_t item) {
  rinzu_managed_slot_t slots[257];
  assert(count <= 257);
  for (size_t i = 0; i < count; ++i) slots[i] = item;
  uint64_t output;
  assert(!rinzu_managed_composite(data, type, -1, slots, count, &output));
  return output;
}
static uint64_t invoke(RinzuContext *context, int operation, size_t result,
                         size_t input, size_t element, uint64_t source, int64_t index,
                         rinzu_managed_slot_t replacement) {
  size_t integer = rinzu_program_schema.types[list_type(context->shared_data,
                                                        RINZU_VALUE_SCALAR)].element;
  uint64_t types[3] = {input, operation == RINZU_DATA_LIST_APPEND ? element : integer, element};
  uint64_t bits[3] = {source, operation == RINZU_DATA_LIST_APPEND ? replacement.bits : (uint64_t)index,
                     replacement.bits};
  list_allocations = list_copy_bytes = 0;
  list_count_active = 1;
  uint64_t output = (uint64_t)rinzu_baseline_data_operation(context, operation, result, types, bits,
                                                          operation == RINZU_DATA_LIST_SET ? 3 : 2);
  list_count_active = 0;
  return output;
}
struct owner_call { RinzuContext *context; size_t list, integer; uint64_t input; };
static void *foreign_owner(void *raw) {
  struct owner_call *call = raw;
  (void)invoke(call->context, RINZU_DATA_LIST_APPEND, call->list, call->list, call->integer,
               call->input, 0, (rinzu_managed_slot_t){7, 0});
  abort();
}
int main(int argc, char **argv) {
  assert(argc == 2); alarm(30);
  RinzuContext context = {0}; context.shared_data = &context.data;
  rinzu_managed_data_t *data = rinzu_context_data(&context);
  rinzu_managed_attach_roots(data, &context.domain_roots, &context.roots);
  rinzu_managed_enter(&context.roots);
  size_t ints = list_type(data, RINZU_VALUE_SCALAR), strings = list_type(data, RINZU_VALUE_STRING);
  size_t integer = data->schema->types[ints].element;
  size_t string = data->schema->types[strings].element;
  size_t option = option_type(data, ints);
  const char *mode = argv[1];
  if (!strcmp(mode, "normal")) {
    const size_t lengths[] = {0, 1, 2, 3, 16, 64, 257};
    for (size_t k = 0; k < sizeof(lengths)/sizeof(*lengths); ++k) {
      size_t n = lengths[k];
      for (int operation = RINZU_DATA_LIST_APPEND; operation <= RINZU_DATA_LIST_REMOVE; ++operation) {
        int64_t indices[] = {-1, 0, (int64_t)(n / 2), n ? (int64_t)n - 1 : 0, (int64_t)n};
        for (size_t at = 0; at < sizeof(indices)/sizeof(*indices); ++at) {
          uint64_t input = make_list(data, ints, n, (rinzu_managed_slot_t){11, 0});
          rinzu_managed_index_resize(data, 4096); /* exclude index capacity growth from allocation proof */
          int append = operation == RINZU_DATA_LIST_APPEND;
          int found = indices[at] >= 0 && (uint64_t)indices[at] < n;
          size_t used = append ? n + 1 : n - (operation == RINZU_DATA_LIST_REMOVE && found);
          uint64_t output = invoke(&context, operation, append ? ints : option, ints, integer,
                                   input, indices[at], (rinzu_managed_slot_t){99, 0});
          size_t expected_allocations = 1 + (!append && found) +
              ((append || found) && used > 1) + !TEST_LIST_DIRECT;
          assert(list_allocations == expected_allocations);
          if (append) {
            size_t copies = TEST_LIST_DIRECT ? (used > 1 ? n : 0)
                                             : n + (used > 1 ? used : 0);
            assert(list_copy_bytes == copies * sizeof(rinzu_managed_slot_t));
          }
          rinzu_managed_value_t *parent = rinzu_managed_find(data, output);
          uint64_t result = output;
          if (!append) {
            assert(parent->has_variant && parent->variant == (unsigned)found);
            assert(parent->count == (size_t)found);
            if (found) result = parent->slots[0].bits;
          }
          uint64_t roots[] = {input, output}; rinzu_managed_publish(&context.roots, roots, 2);
          for (size_t gc = 0; gc < 2; ++gc) {
            rinzu_managed_value_t *original = rinzu_managed_find(data, input);
            assert(original->count == n);
            for (size_t i = 0; i < n; ++i) assert(original->slots[i].bits == 11 && !original->slots[i].reference);
            if (append || found) {
              rinzu_managed_value_t *updated = rinzu_managed_find(data, result);
              assert(updated != original && updated->count == used && updated->type == ints);
              for (size_t i = 0; i < used; ++i) {
                int changed = append ? i == n : operation == RINZU_DATA_LIST_SET && i == (size_t)indices[at];
                assert(updated->slots[i].bits == (uint64_t)(changed ? 99 : 11));
                assert(!updated->slots[i].reference);
              }
            }
            rinzu_managed_collect(data, &context.roots);
          }
        }
      }
    }
  } else if (!strcmp(mode, "references")) {
    uint64_t left = rinzu_managed_string(data, (const unsigned char *)"left", 4);
    uint64_t right = rinzu_managed_string(data, (const unsigned char *)"right", 5);
    uint64_t replacement = rinzu_managed_string(data, (const unsigned char *)"new", 3);
    rinzu_managed_slot_t pair[] = {{left, 1}, {right, 1}};
    uint64_t input; assert(!rinzu_managed_composite(data, strings, -1, pair, 2, &input));
    size_t option_strings = option_type(data, strings);
    uint64_t output = invoke(&context, RINZU_DATA_LIST_SET, option_strings, strings, string,
                             input, 0, (rinzu_managed_slot_t){replacement, 1});
    rinzu_managed_publish(&context.roots, &output, 1);
    rinzu_managed_collect(data, &context.roots);
    assert(!rinzu_managed_lookup(data, input) && !rinzu_managed_lookup(data, left));
    assert(rinzu_managed_find(data, replacement) && rinzu_managed_find(data, right));
    uint64_t list = rinzu_managed_find(data, output)->slots[0].bits;
    rinzu_managed_value_t *value = rinzu_managed_find(data, list);
    assert(value->count == 2 && value->slots[0].bits == replacement && value->slots[1].bits == right);
    assert(value->slots[0].reference && value->slots[1].reference);
    output = invoke(&context, RINZU_DATA_LIST_REMOVE, option_strings, strings, string,
                    list, 1, (rinzu_managed_slot_t){0});
    rinzu_managed_publish(&context.roots, &output, 1);
    rinzu_managed_collect(data, &context.roots);
    assert(!rinzu_managed_lookup(data, right));
    list = rinzu_managed_find(data, output)->slots[0].bits;
    value = rinzu_managed_find(data, list);
    assert(value->count == 1 && value->slots[0].bits == replacement && value->slots[0].reference);
    assert(rinzu_managed_find(data, replacement));
  } else if (!strcmp(mode, "scalar-tags")) {
    uint64_t hidden = rinzu_managed_string(data, (const unsigned char *)"not a reference", 15);
    uint64_t input = make_list(data, ints, 2, (rinzu_managed_slot_t){hidden, 0});
    uint64_t output = invoke(&context, RINZU_DATA_LIST_APPEND, ints, ints, integer, input, 0,
                             (rinzu_managed_slot_t){hidden, 0});
    rinzu_managed_publish(&context.roots, &output, 1);
    rinzu_managed_collect(data, &context.roots);
    assert(!rinzu_managed_lookup(data, hidden));
    rinzu_managed_value_t *value = rinzu_managed_find(data, output);
    assert(value->count == 3);
    for (size_t i = 0; i < 3; ++i) assert(value->slots[i].bits == hidden && !value->slots[i].reference);
  } else {
    uint64_t input = make_list(data, ints, 2, (rinzu_managed_slot_t){11, 0});
    rinzu_managed_index_resize(data, 4096);
    if (!strcmp(mode, "foreign-list") || !strcmp(mode, "foreign-value")) {
      rinzu_managed_data_t other = {0};
      assert(!rinzu_managed_configure_values(&other, data->schema));
      if (!strcmp(mode, "foreign-list")) {
        uint64_t foreign = make_list(&other, ints, 2, (rinzu_managed_slot_t){11, 0});
        (void)invoke(&context, RINZU_DATA_LIST_APPEND, ints, ints, integer, foreign, 0, (rinzu_managed_slot_t){99, 0});
      } else {
        uint64_t local = rinzu_managed_string(data, (const unsigned char *)"local", 5);
        uint64_t foreign = rinzu_managed_string(&other, (const unsigned char *)"foreign", 7);
        input = make_list(data, strings, 2, (rinzu_managed_slot_t){local, 1});
        (void)invoke(&context, RINZU_DATA_LIST_APPEND, strings, strings, string, input, 0, (rinzu_managed_slot_t){foreign, 1});
      }
    } else if (!strcmp(mode, "wrong-result")) {
      (void)invoke(&context, RINZU_DATA_LIST_APPEND, strings, ints, integer, input, 0, (rinzu_managed_slot_t){99, 0});
    } else if (!strcmp(mode, "wrong-element")) {
      uint64_t text = rinzu_managed_string(data, (const unsigned char *)"text", 4);
      input = make_list(data, strings, 2, (rinzu_managed_slot_t){text, 1});
      (void)invoke(&context, RINZU_DATA_LIST_APPEND, strings, strings, integer, input, 0, (rinzu_managed_slot_t){99, 0});
    } else if (!strcmp(mode, "overflow")) {
      rinzu_managed_find(data, input)->count = SIZE_MAX / sizeof(rinzu_managed_slot_t);
      (void)invoke(&context, RINZU_DATA_LIST_APPEND, ints, ints, integer, input, 0, (rinzu_managed_slot_t){99, 0});
    } else if (!strcmp(mode, "owner")) {
      struct owner_call call = {&context, ints, integer, input}; pthread_t thread;
      assert(!pthread_create(&thread, NULL, foreign_owner, &call));
      assert(!pthread_join(thread, NULL));
    } else {
      assert(!strcmp(mode, "oom-meta") || !strcmp(mode, "oom-slots") || !strcmp(mode, "oom-option"));
      list_fail_at = !strcmp(mode, "oom-meta") ? 1 : !strcmp(mode, "oom-slots") ? 2 : 3;
      (void)invoke(&context, RINZU_DATA_LIST_SET, option, ints, integer, input, 1, (rinzu_managed_slot_t){99, 0});
    }
    abort();
  }
  rinzu_managed_publish(&context.roots, NULL, 0);
  rinzu_managed_collect(data, &context.roots);
  assert(!data->objects && !data->heap_bytes);
  rinzu_managed_leave(&context.roots); rinzu_managed_detach_roots(data, &context.domain_roots);
  rinzu_managed_dispose(&context.roots); rinzu_managed_data_dispose(data);
  return 0;
}
