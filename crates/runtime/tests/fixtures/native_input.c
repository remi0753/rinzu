#include <stdlib.h>
static size_t allocation_budget = (size_t)-1;
static void *controlled_calloc(size_t count, size_t size) {
  if (!allocation_budget)
    return NULL;
  if (allocation_budget != (size_t)-1)
    allocation_budget--;
  return calloc(count, size);
}
static inline void *controlled_malloc(size_t size) {
  if (!allocation_budget)
    return NULL;
  if (allocation_budget != (size_t)-1)
    allocation_budget--;
  return malloc(size);
}
#define calloc controlled_calloc
#define malloc controlled_malloc
#include "rinzu/native_input.h"
#undef calloc
#undef malloc
#include <assert.h>
#include <stdio.h>
#include <sys/mman.h>
static const unsigned char bytes[4] = {0, 255, 7, 0};
static void run_dynamic(void) {
  rinzu_managed_data_t data = {0};
  rinzu_managed_roots_t roots = {0};
  uint64_t output = UINT64_MAX;
  assert(rinzu_managed_receive_byte_array(&data, NULL, &output) == RINZU_NATIVE_NULL_BYTE_ARRAY);
  assert(output == UINT64_MAX && !data.objects);
  uint8_t original[] = {0, 255, 128, 0};
  rinzu_native_byte_array_t view = {original, sizeof(original)};
  assert(!rinzu_managed_receive_byte_array(&data, &view, &output));
  memset(original, 90, sizeof(original));
  view.data = NULL; view.length = SIZE_MAX;
  rinzu_managed_enter(&roots); rinzu_managed_publish(&roots, &output, 1);
  rinzu_managed_collect(&data, &roots);
  const unsigned char expected[] = {0, 255, 128, 0};
  rinzu_managed_value_t *value = rinzu_managed_find(&data, output);
  assert(value->length == sizeof(expected) && !memcmp(value->bytes, expected, sizeof(expected)));
  rinzu_managed_leave(&roots); rinzu_managed_collect(&data, &roots);
  assert(!data.objects);
  rinzu_managed_dispose(&roots); rinzu_managed_data_dispose(&data);
}
static void run(void) {
  rinzu_managed_data_t data = {0};
  rinzu_managed_roots_t roots = {0};
  uint64_t output = UINT64_MAX;
  assert(rinzu_managed_receive_string(&data, NULL, &output) == RINZU_NATIVE_NULL_STRING);
  assert(output == UINT64_MAX && data.objects == 0);
  assert(rinzu_managed_receive_string(&data, (const unsigned char *)"\xff", &output) ==
         RINZU_NATIVE_INVALID_UTF8);
  assert(output == UINT64_MAX && data.objects == 0);
  unsigned char original[4] = {0, 255, 7, 0};
  rinzu_managed_receive_bytes(&data, 4, original, &output);
  memset(original, 42, 4);
  assert(!memcmp(rinzu_managed_find(&data, output)->bytes, bytes, 4));
  rinzu_managed_collect(&data, &roots);
  assert(data.objects == 0);
  uint64_t caller = rinzu_managed_string(&data, (const unsigned char *)"caller", 6);
  rinzu_managed_enter(&roots);
  rinzu_managed_publish(&roots, &caller, 1);
  for (int kind = 0; kind < 4; ++kind) {
    uint64_t expected = kind == 0 ? 0 : kind == 1 ? 4 : kind == 2 ? 1 : 13;
    const unsigned char *input = kind == 0   ? NULL
                                 : kind == 1 ? bytes
                                 : kind == 2 ? (const unsigned char *)"A\0ignored"
                                             : (const unsigned char *)"日本語😀";
    rinzu_native_length_callback_t callback =
        rinzu_native_length_prepare(&data, &roots, kind >= 2, expected);
    for (int repeat = 0; repeat < 32; ++repeat)
      assert(callback.callback.entry(input) == expected);
    assert(callback.context->prepared_inputs == 32 && callback.context->body_entries == 32);
    assert(callback.context->copied_bytes == expected * 32);
    rinzu_native_length_release(&callback);
    assert(data.objects == 1 && roots.depth == 1);
    assert(!memcmp(rinzu_managed_find(&data, caller)->bytes, "caller", 6));
  }
  assert(data.collections == 257);
  rinzu_managed_leave(&roots);
  rinzu_managed_collect(&data, &roots);
  assert(data.objects == 0 && roots.depth == 0);
  rinzu_managed_dispose(&roots);
  rinzu_managed_data_dispose(&data);
}
static void *worker(void *unused) {
  (void)unused;
  run_dynamic();
  run();
  return NULL;
}
int main(int argc, char **argv) {
  if (argc > 1) {
    const char *mode = argv[1];
    rinzu_managed_data_t data = {0};
    rinzu_managed_roots_t roots = {0};
    if (!strncmp(mode, "array-", 6)) {
      uint64_t output = UINT64_MAX;
      if (!strcmp(mode, "array-oom")) {
        rinzu_managed_owner(&data);
        rinzu_managed_index_resize(&data, 128);
        uint8_t payload[4096] = {0};
        rinzu_native_byte_array_t view = {payload, sizeof(payload)};
        allocation_budget = 1; /* Metadata succeeds; its payload malloc fails. */
        (void)rinzu_managed_receive_byte_array(&data, &view, &output);
        abort();
      }
      rinzu_native_byte_array_t view = {NULL, !strcmp(mode, "array-size") ? SIZE_MAX : 1};
      if (!strcmp(mode, "array-read") || !strcmp(mode, "array-descriptor") ||
          !strcmp(mode, "array-zero-unreadable")) {
        void *page = mmap(NULL, 16384, PROT_NONE, MAP_PRIVATE | MAP_ANON, -1, 0);
        assert(page != MAP_FAILED);
        view.data = page;
        if (!strcmp(mode, "array-zero-unreadable")) view.length = 0;
        (void)rinzu_managed_receive_byte_array(&data,
            !strcmp(mode, "array-descriptor") ? page : &view, &output);
        if (!strcmp(mode, "array-zero-unreadable")) {
          assert(rinzu_managed_find(&data, output)->length == 0);
          rinzu_managed_collect(&data, &roots); rinzu_managed_data_dispose(&data);
          assert(!munmap(page, 16384)); return 0;
        }
      } else (void)rinzu_managed_receive_byte_array(&data, &view, &output);
      abort();
    }
    rinzu_native_length_callback_t callback = rinzu_native_length_prepare(
        &data, &roots, !strcmp(mode, "string-null"), !strcmp(mode, "size") ? UINT64_MAX : 4);
    if (!strncmp(mode, "oom-", 4))
      allocation_budget = strtoull(mode + 4, NULL, 10);
    rinzu_callback_pointer_u64_t entry = callback.callback.entry;
    if (!strcmp(mode, "closed"))
      rinzu_native_length_release(&callback);
    if (!strcmp(mode, "read")) {
      void *page = mmap(NULL, 16384, PROT_NONE, MAP_PRIVATE | MAP_ANON, -1, 0);
      assert(page != MAP_FAILED);
      (void)entry(page);
    } else if (!strcmp(mode, "zero-no-read")) {
      rinzu_native_length_release(&callback);
      callback = rinzu_native_length_prepare(&data, &roots, 0, 0);
      void *page = mmap(NULL, 16384, PROT_NONE, MAP_PRIVATE | MAP_ANON, -1, 0);
      assert(page != MAP_FAILED);
      assert(callback.callback.entry(page) == 0);
      assert(data.objects == 0);
      rinzu_native_length_release(&callback);
      assert(!munmap(page, 16384));
      return 0;
    } else
      (void)entry(!strcmp(mode, "size") || !strncmp(mode, "oom-", 4) ? bytes : NULL);
    abort();
  }
  run_dynamic(); run();
  pthread_t threads[8];
  for (size_t index = 0; index < 8; ++index)
    assert(!pthread_create(&threads[index], NULL, worker, NULL));
  for (size_t index = 0; index < 8; ++index)
    assert(!pthread_join(threads[index], NULL));
  rinzu_callback_stats_t final = rinzu_callback_entry_stats();
  assert(final.entries == 36 && final.live_connections == 0 && final.in_flight == 0);
  puts("PASS native length Main/Worker input-copy root GC body-entry baseline");
}
