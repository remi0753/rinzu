#include "rinzu/managed_composite.h"
#include <assert.h>
#include <sys/wait.h>
#include <unistd.h>
static const size_t fields[] = {0, 1, 3}, link_fields[] = {2};
static const rinzu_key_variant_t variants[] = {{0, NULL}, {1, link_fields}};
static const rinzu_value_type_t types[] = {
    {.kind = RINZU_VALUE_SCALAR, .scalar_kind = RINZU_SCALAR_I64},
    {.kind = RINZU_VALUE_STRING},
    {.kind = RINZU_VALUE_STATE, .identity = "Node", .fields = fields, .field_count = 3},
    {.kind = RINZU_VALUE_ENUM, .identity = "Link", .variants = variants, .variant_count = 2}};
static uint64_t node(rinzu_managed_data_t *data, uint64_t text, uint64_t none) {
  rinzu_managed_slot_t values[] = {{1, 0}, {text, 1}, {none, 1}};
  uint64_t output;
  assert(!rinzu_managed_composite(data, 2, -1, values, 3, &output));
  return output;
}
static uint64_t some(rinzu_managed_data_t *data, uint64_t value) {
  rinzu_managed_slot_t slot = {value, 1};
  uint64_t output;
  assert(!rinzu_managed_composite(data, 3, 1, &slot, 1, &output));
  return output;
}
static void *foreign(void *context) {
  rinzu_managed_data_t *data = context;
  (void)rinzu_managed_set_state_field(data, 0, 2, 0, (rinzu_managed_slot_t){9, 0});
  return NULL;
}
int main(void) {
  rinzu_value_schema_t schema = {0};
  assert(!rinzu_value_schema_init(&schema, types, 4));
  assert(!schema.equality[2] && !schema.transferable[2] && !schema.keys.allowed[2]);
  assert(!schema.transferable[3]);
  rinzu_managed_data_t data = {0}, other = {0};
  assert(!rinzu_managed_configure_values(&data, &schema));
  assert(!rinzu_managed_configure_values(&other, &schema));
  rinzu_managed_roots_t roots = {0};
  rinzu_managed_enter(&roots);
  uint64_t none;
  assert(!rinzu_managed_composite(&data, 3, 0, NULL, 0, &none));
  uint64_t text = rinzu_managed_string(&data, (const unsigned char *)"old", 3);
  uint64_t first = node(&data, text, none);
  rinzu_managed_publish(&roots, &first, 1);
  rinzu_managed_collect(&data, &roots);
  text = rinzu_managed_string(&data, (const unsigned char *)"young", 5);
  assert(!rinzu_managed_set_state_field(&data, first, 2, 1, (rinzu_managed_slot_t){text, 1}));
  assert(!rinzu_managed_set_state_field(&data, first, 2, 2,
                                        (rinzu_managed_slot_t){some(&data, first), 1}));
  rinzu_managed_collect(&data, &roots);
  rinzu_managed_slot_t slot;
  assert(!rinzu_managed_project(&data, first, 2, -1, 1, &slot));
  assert(rinzu_managed_find(&data, slot.bits)->length == 5);
  assert(!rinzu_managed_composite(&data, 3, 0, NULL, 0, &none));
  uint64_t second = node(&data, slot.bits, none);
  assert(!rinzu_managed_set_state_field(&data, first, 2, 2,
                                        (rinzu_managed_slot_t){some(&data, second), 1}));
  assert(!rinzu_managed_set_state_field(&data, second, 2, 2,
                                        (rinzu_managed_slot_t){some(&data, first), 1}));
  for (int i = 0; i < 100; i++)
    rinzu_managed_collect(&data, &roots);
  assert(rinzu_managed_set_state_field(&other, first, 2, 0, (rinzu_managed_slot_t){99, 0}) ==
         RINZU_VALUE_TYPE);
  assert(rinzu_managed_set_state_field(&data, first, 2, 99, (rinzu_managed_slot_t){99, 0}) ==
         RINZU_VALUE_TYPE);
  assert(rinzu_managed_set_state_field(&data, first, 2, 1, (rinzu_managed_slot_t){99, 0}) ==
         RINZU_VALUE_TYPE);
  assert(!rinzu_managed_project(&data, first, 2, -1, 0, &slot) && slot.bits == 1);
  pid_t child = fork();
  assert(child >= 0);
  if (!child) {
    pthread_t thread;
    assert(!pthread_create(&thread, NULL, foreign, &data));
    (void)pthread_join(thread, NULL);
    _Exit(1);
  }
  int status;
  assert(waitpid(child, &status, 0) == child && WIFEXITED(status) && WEXITSTATUS(status) == 70);
  rinzu_managed_leave(&roots);
  rinzu_managed_collect(&data, &roots);
  assert(!data.objects);
  rinzu_managed_data_dispose(&data);
  rinzu_managed_data_dispose(&other);
  rinzu_managed_dispose(&roots);
  rinzu_value_schema_dispose(&schema);
}
