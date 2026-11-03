#include "rinzu/native_input.h"
#include "rinzu/callback_return.h"
#include <assert.h>
#include <stdio.h>
int main(void) {
  atomic_store(&rinzu_resource_enabled, 1);
  rinzu_managed_data_t data = {0};
  rinzu_managed_roots_t roots = {0};
  rinzu_managed_enter(&roots);
  uint64_t text = rinzu_managed_string(&data, (const unsigned char *)"abc", 3);
  uint64_t bytes = 0;
  assert(!rinzu_managed_bytes(&data, 4, (const unsigned char *)"ABCD", 4, &bytes));
  size_t byte_payload = data.inline_bytes_enabled ? 0 : 5;
  assert(rinzu_resource_counters[RINZU_RESOURCE_MANAGED].live ==
         2 * sizeof(rinzu_managed_value_t) + byte_payload + data.index_accounted_bytes);
  assert(data.heap_bytes == 2 * sizeof(rinzu_managed_value_t) + byte_payload);
  uint64_t both[] = {text, bytes};
  rinzu_managed_publish(&roots, both, 2);
  data.gc.stress = 1; /* The next assertions require actual moving collections. */
  rinzu_managed_collect(&data, &roots);
  /* GC moves both payloads out of metadata, charging their actual storage. */
  assert(data.heap_bytes == 2 * sizeof(rinzu_managed_value_t) + 4 + 5);
  assert(rinzu_resource_counters[RINZU_RESOURCE_MANAGED].live ==
         data.heap_bytes + data.index_accounted_bytes);
  assert(!memcmp(rinzu_managed_find(&data, bytes)->bytes, "ABCD", 5));
  rinzu_managed_publish(&roots, &text, 1);
  rinzu_managed_collect(&data, &roots);
  assert(rinzu_resource_counters[RINZU_RESOURCE_MANAGED].live ==
         sizeof(rinzu_managed_value_t) + 4 + data.index_accounted_bytes);
  assert(data.heap_bytes == sizeof(rinzu_managed_value_t) + 4);
  data.gc.stress = 0; /* Normal survivor collections retain their payload storage. */
  uint64_t allocated = rinzu_resource_counters[RINZU_RESOURCE_MANAGED].allocated;
  for (int i = 0; i < 3; i++)
    rinzu_managed_collect(&data, &roots);
  assert(rinzu_resource_counters[RINZU_RESOURCE_MANAGED].allocated == allocated);
  uint64_t input = 0;
  assert(!rinzu_managed_receive_string(&data, (const unsigned char *)"ok", &input));
  const unsigned char invalid[] = {255, 0};
  assert(rinzu_managed_receive_string(&data, invalid, &input) == RINZU_NATIVE_INVALID_UTF8);
  assert(rinzu_resource_counters[RINZU_RESOURCE_NATIVE_INPUT].live == 0);
  assert(rinzu_resource_counters[RINZU_RESOURCE_NATIVE_INPUT].allocated == 5);
  assert(rinzu_resource_counters[RINZU_RESOURCE_NATIVE_INPUT].released == 5);
  rinzu_managed_leave(&roots);
  data.gc.stress = 1;
  rinzu_managed_collect(&data, &roots);
  rinzu_managed_dispose(&roots);
  rinzu_managed_data_dispose(&data);
  assert(rinzu_resource_counters[RINZU_RESOURCE_MANAGED].live == 0);
  assert(rinzu_resource_counters[RINZU_RESOURCE_MANAGED_OBJECTS].live == 0);
  const unsigned char *normal =
      rinzu_callback_return_buffer_copy((const unsigned char *)"ab", 2, 2, 1, 0);
  const unsigned char *fallback = rinzu_callback_return_buffer_copy(NULL, 0, 0, 0, 1);
  assert(!memcmp(normal, "ab\0", 3) && !fallback[0]);
  rinzu_callback_return_stats_t returns = rinzu_callback_return_buffer_stats();
  assert(returns.normal_bytes == 3 && returns.fallback_bytes == 1 && returns.bytes == 4);
  assert(returns.metadata_bytes == 2 * sizeof(rinzu_callback_return_node_t));
  puts("resource accounting passed");
  return 0;
}
