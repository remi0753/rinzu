#include "rinzu/native_input.h"
#include <assert.h>
#include <stdio.h>
/* Name placeholders are replaced with symbols from the emitted object. */
extern uint64_t STUB_mixed(uint64_t, const uint64_t *);
extern uint64_t STUB_floating(uint64_t, const uint64_t *);
extern uint64_t STUB_signed8(uint64_t, const uint64_t *);
extern uint64_t STUB_signed16(uint64_t, const uint64_t *);
extern uint64_t STUB_signed32(uint64_t, const uint64_t *);
extern uint64_t STUB_signed64(uint64_t, const uint64_t *);
extern uint64_t STUB_unsigned8(uint64_t, const uint64_t *);
extern uint64_t STUB_unsigned16(uint64_t, const uint64_t *);
extern uint64_t STUB_unsigned32(uint64_t, const uint64_t *);
extern uint64_t STUB_unsigned64(uint64_t, const uint64_t *);
extern uint64_t STUB_echoFloat(uint64_t, const uint64_t *);
extern uint64_t STUB_echoDouble(uint64_t, const uint64_t *);
extern uint64_t STUB_boolean(uint64_t, const uint64_t *);
extern uint64_t STUB_empty(uint64_t, const uint64_t *);
extern uint64_t STUB_count(uint64_t, const uint64_t *);
extern uint64_t STUB_invoke(uint64_t, const uint64_t *);
static void validate(void) {
  assert(STUB_signed8((uintptr_t)signed8, (uint64_t[]){(uint64_t)INT8_MIN}) == (uint64_t)INT8_MIN);
  assert(STUB_signed16((uintptr_t)signed16, (uint64_t[]){(uint64_t)INT16_MIN}) ==
         (uint64_t)INT16_MIN);
  assert(STUB_signed32((uintptr_t)signed32, (uint64_t[]){(uint64_t)INT32_MIN}) ==
         (uint64_t)INT32_MIN);
  assert(STUB_signed64((uintptr_t)signed64, (uint64_t[]){(uint64_t)INT64_MIN}) ==
         (uint64_t)INT64_MIN);
  assert(STUB_unsigned8((uintptr_t)unsigned8, (uint64_t[]){UINT8_MAX}) == UINT8_MAX);
  assert(STUB_unsigned16((uintptr_t)unsigned16, (uint64_t[]){UINT16_MAX}) == UINT16_MAX);
  assert(STUB_unsigned32((uintptr_t)unsigned32, (uint64_t[]){UINT32_MAX}) == UINT32_MAX);
  assert(STUB_unsigned64((uintptr_t)unsigned64, (uint64_t[]){UINT64_MAX}) == UINT64_MAX);
  uint64_t float_bits[] = {UINT64_C(0x80000000), UINT64_C(0x7fc01234), UINT64_C(0x3f800001),
                           UINT64_C(0x7f7fffff)};
  uint64_t double_bits[] = {UINT64_C(0x8000000000000000), UINT64_C(0x7ff8123400004321),
                            UINT64_C(0x3ff0000000000001), UINT64_C(0x7fefffffffffffff)};
  for (size_t index = 0; index < 4; ++index) {
    assert(STUB_echoFloat((uintptr_t)echo_float, &float_bits[index]) == float_bits[index]);
    assert(STUB_echoDouble((uintptr_t)echo_double, &double_bits[index]) == double_bits[index]);
  }
  assert(STUB_boolean((uintptr_t)boolean, (uint64_t[]){0}) == 0);
  assert(STUB_boolean((uintptr_t)boolean, (uint64_t[]){1}) == 1);
  assert(STUB_mixed((uintptr_t)mixed,
                    (uint64_t[]){(uint64_t)INT8_MIN, UINT8_MAX, (uint64_t)INT16_MIN, UINT16_MAX,
                                 (uint64_t)INT32_MIN, UINT32_MAX, (uint64_t)INT64_MIN, UINT64_MAX,
                                 UINT64_C(0x3f800001), UINT64_C(0x8000000000000000), 1}) ==
         UINT64_MAX);
  uint64_t words[10];
  for (size_t index = 0; index < 10; ++index) {
    if (index % 2 == 0) {
      float value = (float)(index + 1);
      uint32_t bits;
      memcpy(&bits, &value, 4);
      words[index] = bits;
    } else {
      double value = (double)(index + 1);
      memcpy(&words[index], &value, 8);
    }
  }
  double expected = 55;
  uint64_t expected_bits;
  memcpy(&expected_bits, &expected, 8);
  assert(STUB_floating((uintptr_t)floating, words) == expected_bits);
  rinzu_managed_data_t data = {0};
  rinzu_managed_roots_t roots = {0};
  rinzu_native_length_callback_t callback = rinzu_native_length_prepare(&data, &roots, 0, 4);
  const unsigned char input[4] = {0, 255, 7, 0};
  assert(STUB_invoke((uintptr_t)invoke,
                     (uint64_t[]){(uintptr_t)callback.callback.entry, (uintptr_t)input}) == 4);
  assert(callback.context->prepared_inputs == 1 && callback.context->body_entries == 1);
  assert(data.objects == 0 && roots.depth == 0 && data.collections == 2);
  rinzu_native_length_release(&callback);
  rinzu_managed_data_dispose(&data);
  rinzu_managed_dispose(&roots);
}
static void *worker(void *unused) {
  (void)unused;
  validate();
  return NULL;
}
int main(void) {
  assert(STUB_empty((uintptr_t)empty, NULL) == 0);
  assert(STUB_count((uintptr_t)count, NULL) == 1);
  validate();
  pthread_t workers[8];
  for (size_t index = 0; index < 8; ++index)
    assert(!pthread_create(&workers[index], NULL, worker, NULL));
  for (size_t index = 0; index < 8; ++index)
    assert(!pthread_join(workers[index], NULL));
  assert(rinzu_callback_entry_stats().live_connections == 0);
  puts("PASS physical native ABI AOT/Main/Worker scalars/stack/float/Unit/one-pointer-callback");
}
