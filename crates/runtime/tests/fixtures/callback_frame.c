#include "rinzu/callback_entry.h"
#include <assert.h>
#include <limits.h>
#include <pthread.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef double (*MixedEntry)(int8_t, float, uint16_t, double, int32_t, uint64_t, const void *,
                             int16_t, uint8_t, int64_t, bool, uint32_t, int8_t, uint16_t,
                             const void *, float, double, float, double, float, double, float,
                             double, float);
static const unsigned char bytes[4] = {0, 255, 7, 0};
static const uint64_t result_bits = UINT64_C(0xfff8010203040506);
static const unsigned kinds[24] = {0, 8, 5,  9, 2, 7, 11, 1, 4, 3, 10, 6,
                                   0, 5, 11, 8, 9, 8, 9,  8, 9, 8, 9,  8};
typedef struct {
  pthread_t origin;
  uint64_t identity;
  size_t calls;
  int nested;
  rinzu_callback_frame_entry_t callback;
} Context;
static double f64(uint64_t bits) {
  double value;
  memcpy(&value, &bits, 8);
  return value;
}
static float f32(uint32_t bits) {
  float value;
  memcpy(&value, &bits, 4);
  return value;
}
static uint64_t call_mixed(const void *pointer) {
  MixedEntry entry = (MixedEntry)pointer;
  double result =
      entry(INT8_MIN, f32(0x80000000), UINT16_MAX, f64(UINT64_C(0x7ff8123456789abc)), INT32_MIN,
            UINT64_MAX, bytes, INT16_MIN, UINT8_MAX, INT64_MIN, true, UINT32_MAX, -7, 64523, bytes,
            f32(0x7f800000), f64(UINT64_C(0xfff0000000000000)), f32(0x7fc12345),
            f64(UINT64_C(0x8000000000000000)), 0, 42.5, 1.25, 2.5, -2.25);
  uint64_t bits;
  memcpy(&bits, &result, 8);
  return bits;
}
static uint64_t handler(void *raw, const rinzu_callback_frame_t *frame) {
  Context *context = raw;
  assert(pthread_equal(context->origin, pthread_self()));
  const uint64_t expected[24] = {(uint64_t)(int64_t)INT8_MIN,
                                 0x80000000,
                                 UINT16_MAX,
                                 UINT64_C(0x7ff8123456789abc),
                                 (uint64_t)(int64_t)INT32_MIN,
                                 UINT64_MAX,
                                 (uintptr_t)bytes,
                                 (uint64_t)(int64_t)INT16_MIN,
                                 UINT8_MAX,
                                 (uint64_t)INT64_MIN,
                                 1,
                                 UINT32_MAX,
                                 (uint64_t)(int64_t)-7,
                                 64523,
                                 (uintptr_t)bytes,
                                 0x7f800000,
                                 UINT64_C(0xfff0000000000000),
                                 0x7fc12345,
                                 UINT64_C(0x8000000000000000),
                                 0,
                                 UINT64_C(0x4045400000000000),
                                 0x3fa00000,
                                 UINT64_C(0x4004000000000000),
                                 0xc0100000};
  rinzu_callback_abi_cursor_t cursor = {0};
  for (size_t index = 0; index < 24; index++)
    assert(rinzu_callback_frame_word(frame, &cursor, kinds[index]) == expected[index]);
  assert(cursor.integers == 8 && cursor.floats == 8 && cursor.stack == 44);
  context->calls++;
  if (context->nested == 1) {
    context->nested = 2;
    assert(call_mixed(context->callback.entry) == (result_bits ^ context->identity));
    assert(!rinzu_callback_entry_settled(context->callback.control));
  } else if (context->nested == 2) {
    assert(!rinzu_callback_entry_close(context->callback.control));
    assert(rinzu_callback_entry_stats().in_flight == 2);
  }
  return result_bits ^ context->identity;
}
static void prepare_unopened(Context *context, uint64_t identity) {
  *context = (Context){.origin = pthread_self(), .identity = identity};
  context->callback = rinzu_callback_frame_prepare_deferred(handler, context);
  assert(!rinzu_callback_entry_settled(context->callback.control));
}
static void prepare(Context *context, uint64_t identity) {
  prepare_unopened(context, identity);
  rinzu_callback_entry_open(context->callback.control);
}
static void *wrong_thread(void *raw) {
  Context *context = raw;
  (void)call_mixed(context->callback.entry);
  abort();
}
static void *wrong_thread_open(void *raw) {
  Context *context = raw;
  rinzu_callback_entry_open(context->callback.control);
  abort();
}
enum { WORKERS = 8, PER_WORKER = 129, MAIN_COUNT = 1031 };
typedef struct {
  size_t index;
  const void **entries;
} Worker;
static void *worker(void *raw) {
  Worker *worker = raw;
  for (size_t index = 0; index < PER_WORKER; index++) {
    Context context;
    prepare(&context, worker->index * PER_WORKER + index);
    worker->entries[index] = context.callback.entry;
    assert(call_mixed(context.callback.entry) == (result_bits ^ context.identity));
    assert(rinzu_callback_entry_close(context.callback.control));
    assert(context.calls == 1);
  }
  return NULL;
}
static uint64_t constant_handler(void *context, const rinzu_callback_frame_t *frame) {
  assert(frame);
  return *(const uint64_t *)context;
}
/* Each declaration consumes the same precompiled return bank with its real
   fixed ABI. Zero parameters never read the saved native stack. */
#define CHECK_RESULT(type, bits, expected)                                                         \
  do {                                                                                             \
    uint64_t value = (bits);                                                                       \
    rinzu_callback_frame_entry_t callback =                                                        \
        rinzu_callback_frame_prepare(constant_handler, &value);                                    \
    type (*entry)(void) = (type (*)(void))callback.entry;                                          \
    assert(entry() == (expected));                                                                 \
    assert(rinzu_callback_entry_close(callback.control));                                          \
  } while (0)
static void result_types(void) {
  CHECK_RESULT(int8_t, (uint64_t)(int64_t)INT8_MIN, INT8_MIN);
  CHECK_RESULT(int16_t, (uint64_t)(int64_t)INT16_MIN, INT16_MIN);
  CHECK_RESULT(int32_t, (uint64_t)(int64_t)INT32_MIN, INT32_MIN);
  CHECK_RESULT(int64_t, (uint64_t)INT64_MIN, INT64_MIN);
  CHECK_RESULT(uint8_t, UINT8_MAX, UINT8_MAX);
  CHECK_RESULT(uint16_t, UINT16_MAX, UINT16_MAX);
  CHECK_RESULT(uint32_t, UINT32_MAX, UINT32_MAX);
  CHECK_RESULT(uint64_t, UINT64_MAX, UINT64_MAX);
  CHECK_RESULT(bool, 1, true);
  CHECK_RESULT(const void *, (uintptr_t)bytes, bytes);
  uint64_t bits = 0xffc12345;
  rinzu_callback_frame_entry_t callback = rinzu_callback_frame_prepare(constant_handler, &bits);
  float result = ((float (*)(void))callback.entry)();
  uint32_t actual;
  memcpy(&actual, &result, 4);
  assert(actual == bits);
  assert(rinzu_callback_entry_close(callback.control));
  bits = result_bits;
  callback = rinzu_callback_frame_prepare(constant_handler, &bits);
  double wide = ((double (*)(void))callback.entry)();
  uint64_t wide_bits;
  memcpy(&wide_bits, &wide, 8);
  assert(wide_bits == bits);
  assert(rinzu_callback_entry_close(callback.control));
  bits = 0;
  callback = rinzu_callback_frame_prepare(constant_handler, &bits);
  ((void (*)(void))callback.entry)();
  assert(rinzu_callback_entry_close(callback.control));
}
int main(int argc, char **argv) {
  const char *mode = argc > 1 ? argv[1] : "normal";
  if (!strcmp(mode, "oom-control"))
    rinzu_callback_test_fail_allocation_after(0);
  if (!strcmp(mode, "oom-block"))
    rinzu_callback_test_fail_allocation_after(1);
  if (!strcmp(mode, "resource"))
    rinzu_callback_test_fail_mapping(1);
  if (!strcmp(mode, "permission"))
    rinzu_callback_test_fail_mapping(2);
  if (!strcmp(mode, "merged-control"))
    rinzu_callback_test_merge_control_region();
  if (!strcmp(mode, "null-handler")) {
    uint64_t context = 0;
    (void)rinzu_callback_frame_prepare(NULL, &context);
    abort();
  }
  if (!strcmp(mode, "null-context")) {
    (void)rinzu_callback_frame_prepare(handler, NULL);
    abort();
  }
  if (!strcmp(mode, "bad-kind") || !strcmp(mode, "null-stack") || !strcmp(mode, "stack-overflow")) {
    rinzu_callback_frame_t frame = {0};
    rinzu_callback_abi_cursor_t cursor = {0};
    unsigned kind = 12;
    if (!strcmp(mode, "null-stack")) {
      cursor.integers = 8;
      kind = 11;
    }
    if (!strcmp(mode, "stack-overflow")) {
      cursor.integers = 8;
      cursor.stack = SIZE_MAX;
      frame.stack = bytes;
      kind = 11;
    }
    (void)rinzu_callback_frame_word(&frame, &cursor, kind);
    abort();
  }
  Context first;
  prepare_unopened(&first, 42);
  if (!strcmp(mode, "prepared")) {
    (void)call_mixed(first.callback.entry);
    abort();
  }
  if (!strcmp(mode, "prepared-close")) {
    assert(rinzu_callback_entry_close(first.callback.control));
    assert(rinzu_callback_entry_settled(first.callback.control));
    assert(rinzu_callback_entry_stats().live_connections == 0);
    (void)call_mixed(first.callback.entry);
    abort();
  }
  if (!strcmp(mode, "wrong-thread-open")) {
    pthread_t thread;
    assert(!pthread_create(&thread, NULL, wrong_thread_open, &first));
    assert(!pthread_join(thread, NULL));
    abort();
  }
  if (!strcmp(mode, "null-open")) {
    rinzu_callback_entry_open(NULL);
    abort();
  }
  rinzu_callback_entry_open(first.callback.control);
  if (!strcmp(mode, "double-open")) {
    rinzu_callback_entry_open(first.callback.control);
    abort();
  }
  if (!strcmp(mode, "reopen")) {
    assert(rinzu_callback_entry_close(first.callback.control));
    rinzu_callback_entry_open(first.callback.control);
    abort();
  }
  if (!strcmp(mode, "closed")) {
    assert(rinzu_callback_entry_close(first.callback.control));
    first.calls = SIZE_MAX;
    (void)call_mixed(first.callback.entry);
    abort();
  }
  if (!strcmp(mode, "wrong-thread")) {
    pthread_t thread;
    assert(!pthread_create(&thread, NULL, wrong_thread, &first));
    assert(!pthread_join(thread, NULL));
    abort();
  }
  first.nested = 1;
  assert(call_mixed(first.callback.entry) == (result_bits ^ 42));
  assert(first.calls == 2 && rinzu_callback_entry_settled(first.callback.control));
  assert(rinzu_callback_entry_stats().live_connections == 0);
  result_types();
  Context *contexts = calloc(MAIN_COUNT, sizeof(*contexts));
  assert(contexts);
  for (size_t index = 0; index < MAIN_COUNT; index++)
    prepare(&contexts[index], index);
  for (unsigned repeat = 0; repeat < 3; repeat++)
    for (size_t index = 0; index < MAIN_COUNT; index++)
      assert(call_mixed(contexts[index].callback.entry) == (result_bits ^ index));
  const void *main_entries[MAIN_COUNT];
  for (size_t index = 0; index < MAIN_COUNT; index++) {
    main_entries[index] = contexts[index].callback.entry;
    assert(rinzu_callback_entry_close(contexts[index].callback.control));
    assert(contexts[index].calls == 3);
    for (size_t previous = 0; previous < index; previous++)
      assert(main_entries[index] != main_entries[previous]);
  }
  free(contexts);
  pthread_t threads[WORKERS];
  Worker workers[WORKERS];
  const void *entries[WORKERS * PER_WORKER];
  for (size_t index = 0; index < WORKERS; index++) {
    workers[index] = (Worker){index, &entries[index * PER_WORKER]};
    assert(!pthread_create(&threads[index], NULL, worker, &workers[index]));
  }
  for (size_t index = 0; index < WORKERS; index++)
    assert(!pthread_join(threads[index], NULL));
  for (size_t index = 0; index < WORKERS * PER_WORKER; index++) {
    assert(entries[index] != first.callback.entry);
    for (size_t previous = 0; previous < index; previous++)
      assert(entries[index] != entries[previous]);
    for (size_t main = 0; main < MAIN_COUNT; main++)
      assert(entries[index] != main_entries[main]);
  }
  rinzu_callback_stats_t final = rinzu_callback_entry_stats();
  assert(final.entries == MAIN_COUNT + WORKERS * PER_WORKER + 14);
  assert(final.blocks == (final.entries + 511) / 512 &&
         final.virtual_bytes == final.blocks * 32768);
  assert(final.live_connections == 0 && final.in_flight == 0);
  assert(final.invocations == MAIN_COUNT * 3 + WORKERS * PER_WORKER + 15);
  printf("PASS runtime generic callback entries=%zu blocks=%zu connections=%zu in_flight=%zu "
         "invocations=%zu\n",
         final.entries, final.blocks, final.live_connections, final.in_flight, final.invocations);
}
