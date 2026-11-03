#include <stdlib.h>
static size_t allocations;
static void *counted_calloc(size_t count, size_t size) {
  allocations++;
  return calloc(count, size);
}
#define calloc counted_calloc
#include "rinzu/native_input.h"
#include <assert.h>
#include <stdio.h>

int main(void) {
  atomic_store(&rinzu_resource_enabled, 1);
  const size_t string_field[] = {0};
  const rinzu_key_variant_t variants[] = {{.count = 0}, {.types = string_field, .count = 1}};
  const rinzu_value_type_t types[] = {{.kind = RINZU_VALUE_STRING},
                                      {.kind = RINZU_VALUE_SCALAR, .scalar_kind = RINZU_SCALAR_I64},
                                      {.kind = RINZU_VALUE_ENUM,
                                       .identity = "Option<String>",
                                       .variants = variants,
                                       .variant_count = 2}};
  rinzu_value_schema_t schema = {0};
  assert(!rinzu_value_schema_init(&schema, types, 3));
  rinzu_managed_data_t data = {0}, foreign = {0};
  rinzu_managed_roots_t roots = {0};
  assert(!rinzu_managed_configure_values(&data, &schema));
  assert(!rinzu_managed_configure_values(&foreign, &schema));
  rinzu_managed_index_resize(&data, 256);
  rinzu_managed_enter(&roots);
  const unsigned char *texts[] = {(const unsigned char *)"",
                                  (const unsigned char *)"A\0B",
                                  (const unsigned char *)"123456789012345",
                                  (const unsigned char *)"1234567890123456",
                                  (const unsigned char *)"日本語🙂",
                                  (const unsigned char *)"e\xcc\x81"};
  const size_t lengths[] = {0, 3, 15, 16, 13, 3};
  const size_t scalars[] = {0, 3, 15, 16, 4, 2};
  uint64_t handles[8] = {0};
  for (size_t i = 0; i < 6; i++) {
    size_t before = allocations;
    handles[i] = rinzu_managed_string(&data, texts[i], lengths[i]);
    rinzu_managed_value_t *value = rinzu_managed_find(&data, handles[i]);
    assert(value->length == lengths[i] && value->scalar_length == scalars[i]);
    assert(!memcmp(value->bytes, texts[i], lengths[i]) && !value->bytes[lengths[i]]);
    assert(allocations - before == (lengths[i] < sizeof(value->inline_bytes) ? 1 : 2));
    assert(!rinzu_managed_slot_valid(&data, 0, (rinzu_managed_slot_t){handles[i], 1}));
  }
  rinzu_managed_slot_t child = {handles[4], 1};
  size_t before = allocations;
  assert(!rinzu_managed_composite(&data, 2, 1, &child, 1, &handles[6]));
  assert(allocations == before + 1);
  rinzu_managed_value_t *parent = rinzu_managed_find(&data, handles[6]);
  assert(parent->slots == &parent->inline_slot);

  const unsigned char invalid[][5] = {
      {0xff}, {0xc0, 0x80}, {0xed, 0xa0, 0x80}, {0xf4, 0x90, 0x80, 0x80}, {0xe2, 0x82}};
  const size_t invalid_lengths[] = {1, 2, 3, 4, 2};
  for (size_t i = 0; i < 5; i++) {
    assert(rinzu_utf8_scalar_length(invalid[i], invalid_lengths[i]) == SIZE_MAX);
    uint64_t bad = rinzu_managed_string(&data, invalid[i], invalid_lengths[i]);
    assert(rinzu_managed_slot_valid(&data, 0, (rinzu_managed_slot_t){bad, 1}) == RINZU_VALUE_TYPE);
  }
  uint64_t foreign_text = rinzu_managed_string(&foreign, (const unsigned char *)"foreign", 7);
  before = allocations;
  uint64_t output = 0;
  child.bits = foreign_text;
  assert(rinzu_managed_composite(&data, 2, 1, &child, 1, &output) == RINZU_VALUE_TYPE);
  assert(allocations == before);
  assert(!rinzu_managed_slot_valid(&data, 1, (rinzu_managed_slot_t){foreign_text, 0}));

  unsigned char native[] = "日本語🙂";
  before = allocations;
  assert(!rinzu_managed_receive_string(&data, native, &handles[7]));
  assert(allocations == before + 2); /* snapshot + metadata, no second payload copy */
  memset(native, 'x', sizeof(native) - 1);
  size_t objects = data.objects;
  assert(rinzu_managed_receive_string(&data, invalid[0], &output) == RINZU_NATIVE_INVALID_UTF8);
  assert(data.objects == objects);
  assert(rinzu_resource_counters[RINZU_RESOURCE_NATIVE_INPUT].live == 0);
  rinzu_managed_publish(&roots, handles, 8);
  data.gc.stress = 1; /* This fixture specifically verifies payload relocation. */
  for (int repeat = 0; repeat < 3; repeat++) {
    unsigned char *old_bytes = rinzu_managed_find(&data, handles[4])->bytes;
    rinzu_managed_slot_t *old_slots = rinzu_managed_find(&data, handles[6])->slots;
    rinzu_managed_collect(&data, &roots);
    assert(data.objects == 8);
    parent = rinzu_managed_find(&data, handles[6]);
    assert(parent->slots != old_slots && parent->slots[0].bits == handles[4]);
    for (size_t i = 0; i < 6; i++) {
      rinzu_managed_value_t *value = rinzu_managed_find(&data, handles[i]);
      assert(value->scalar_length == scalars[i]);
      assert(!memcmp(value->bytes, texts[i], lengths[i]));
    }
    assert(rinzu_managed_find(&data, handles[4])->bytes != old_bytes);
    rinzu_managed_value_t *received = rinzu_managed_string_value(&data, handles[7]);
    assert(received->scalar_length == 4 && !memcmp(received->bytes, texts[4], 13));
    size_t accounted = data.heap_bytes + data.index_accounted_bytes +
                       foreign.heap_bytes + foreign.index_accounted_bytes;
    assert(rinzu_resource_counters[RINZU_RESOURCE_MANAGED].live == accounted);
  }
  rinzu_managed_leave(&roots);
  rinzu_managed_collect(&data, &roots);
  assert(!data.objects && !data.heap_bytes);
  rinzu_managed_dispose(&roots);
  rinzu_managed_data_dispose(&data);
  rinzu_managed_data_dispose(&foreign);
  rinzu_value_schema_dispose(&schema);
  assert(!rinzu_resource_counters[RINZU_RESOURCE_MANAGED].live);
  puts("PASS cached UTF-8, inline payloads, owned native input, moving GC, Domain isolation");
}
