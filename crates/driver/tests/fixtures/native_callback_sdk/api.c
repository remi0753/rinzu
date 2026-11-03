#include "api.h"
#include <assert.h>
#include <string.h>
static unsigned char handle;
static _Thread_local uint64_t touches, ticks, bodies;
uint64_t cb_inner(void *data, uint64_t (*callback)(void *, const char *)) {
  assert(data && callback);
  uint64_t result = 0;
  for (int index = 0; index < 4; index++)
    result = callback(data, "inner\0unused");
  return result;
}
int32_t cb_touch(void *input, int32_t value) {
  assert(input == &handle);
  touches++;
  return value;
}
void cb_tick(void) {
  ticks++;
}
void cb_body(void) {
  bodies++;
}
int32_t cb_unit(void (*callback)(void)) {
  uint64_t before = ticks;
  for (int index = 0; index < 4; index++)
    callback();
  assert(ticks == before + 4);
  return 17;
}
int32_t cb_raw(int32_t value) {
  return value + 7;
}
int32_t cb_run_raw(int32_t (*callback)(int32_t)) {
  return callback(16);
}
int32_t cb_no_data(int32_t (*callback)(void *)) {
  return callback(NULL);
}
uint64_t cb_probe(void *first, void *same, void *second, void *event, void *other,
                  ProbeCallback callback, int32_t mode) {
  void *tokens[5] = {first, same, second, event, other};
  for (int index = 0; index < 5; index++) {
    assert(tokens[index]);
    for (int prior = 0; prior < index; prior++)
      assert(tokens[index] != tokens[prior]);
  }
  struct Packet packet = {.narrow = INT8_MIN, .wide = UINT64_MAX, .okay = 1, .large = 42.5};
  uint32_t nan = 0x7fc12345;
  memcpy(&packet.small, &nan, 4);
  unsigned char bytes[4] = {0, 255, 7, 0};
  const char *text = "日本語😀\0ignored";
  uint64_t before = touches, body_before = bodies, result = 0;
  for (int index = 0; index < 4; index++) {
    int8_t narrow = mode == 6 ? 99 : mode == 7 ? 98 : mode == 8 ? 97 : -7;
    _Alignas(struct Packet) unsigned char bad_packet[sizeof(packet)];
    memcpy(bad_packet, &packet, sizeof(packet));
    bad_packet[__builtin_offsetof(struct Packet, okay)] = 2;
    result = callback(mode == 5 ? NULL : second, same, first, event, other, bytes,
                      mode == 2   ? NULL
                      : mode == 3 ? "\xff"
                                  : text,
                      mode == 4    ? NULL
                      : mode == 10 ? (const struct Packet *)bad_packet
                                   : &packet,
                      mode == 9 ? NULL : &handle, narrow, UINT64_MAX, -0.0f, 42.5, 1);
    assert(result == (mode == 6 ? 888 : (mode >= 2 && mode <= 10) ? 901 : 777));
    memset(bytes, 42, sizeof(bytes));
    bytes[0] = 0;
    bytes[1] = 255;
    bytes[2] = 7;
    bytes[3] = 0;
  }
  assert(touches - before ==
         (mode == 0 || mode == 1 || mode == 6 || mode == 7 || mode == 8 ? 4 : 0));
  assert(bodies - body_before ==
         (mode == 0 || mode == 1 || mode == 6 || mode == 7 || mode == 8 ? 4 : 0));
  return result;
}
