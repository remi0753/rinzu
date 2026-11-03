#ifndef RECORD_FIXTURE_H
#define RECORD_FIXTURE_H
#include <stdint.h>
struct Packet {
  int8_t a;
  int16_t b;
  int32_t c;
  int64_t d;
  uint8_t e;
  uint16_t f;
  uint32_t g;
  uint64_t h;
  float i;
  double j;
  _Bool k;
};
int32_t record_check(const struct Packet *value, _Bool nan);
const struct Packet *record_result(int32_t kind);
const struct Packet *record_callback(const struct Packet *value,
                                     uint64_t (*callback)(const uint8_t *));
void record_mutate(void);
void record_fixture_cleanup(void);
#endif
