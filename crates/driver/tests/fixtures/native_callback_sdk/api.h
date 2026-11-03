#ifndef CALLBACK_FIXTURE_H
#define CALLBACK_FIXTURE_H
#include <stdint.h>
struct Packet {
  int8_t narrow;
  uint64_t wide;
  _Bool okay;
  float small;
  double large;
};
typedef uint64_t (*ProbeCallback)(void *, void *, void *, void *, void *, const uint8_t *,
                                  const char *, const struct Packet *, void *, int8_t, uint64_t,
                                  float, double, _Bool);
uint64_t cb_probe(void *, void *, void *, void *, void *, ProbeCallback, int32_t);
uint64_t cb_inner(void *, uint64_t (*)(void *, const char *));
int32_t cb_touch(void *, int32_t);
int32_t cb_unit(void (*)(void));
void cb_tick(void);
void cb_body(void);
int32_t cb_raw(int32_t);
int32_t cb_run_raw(int32_t (*)(int32_t));
int32_t cb_no_data(int32_t (*)(void *));
#endif
