#include <assert.h>
#include <stdint.h>
#include <string.h>
uint64_t mixed(int8_t a, uint8_t b, int16_t c, uint16_t d, int32_t e, uint32_t f, int64_t g,
               uint64_t h, float i, double j, _Bool k) {
  assert(a == INT8_MIN && b == UINT8_MAX && c == INT16_MIN && d == UINT16_MAX);
  assert(e == INT32_MIN && f == UINT32_MAX && g == INT64_MIN && h == UINT64_MAX);
  uint32_t float_bits;
  uint64_t double_bits;
  memcpy(&float_bits, &i, 4);
  memcpy(&double_bits, &j, 8);
  assert(float_bits == UINT32_C(0x3f800001) && double_bits == UINT64_C(0x8000000000000000) && k);
  return UINT64_MAX;
}
double floating(float a, double b, float c, double d, float e, double f, float g, double h, float i,
                double j) {
  assert(a == 1 && b == 2 && c == 3 && d == 4 && e == 5 && f == 6 && g == 7 && h == 8 && i == 9 &&
         j == 10);
  return a + b + c + d + e + f + g + h + i + j;
}
int8_t signed8(int8_t value) {
  return value;
}
int16_t signed16(int16_t value) {
  return value;
}
int32_t signed32(int32_t value) {
  return value;
}
int64_t signed64(int64_t value) {
  return value;
}
uint8_t unsigned8(uint8_t value) {
  return value;
}
uint16_t unsigned16(uint16_t value) {
  return value;
}
uint32_t unsigned32(uint32_t value) {
  return value;
}
uint64_t unsigned64(uint64_t value) {
  return value;
}
float echo_float(float value) {
  return value;
}
double echo_double(double value) {
  return value;
}
_Bool boolean(_Bool value) {
  return value;
}
static uint32_t calls;
void empty(void) {
  calls++;
}
uint32_t count(void) {
  return calls;
}
uint64_t invoke(uint64_t (*callback)(const unsigned char *), const unsigned char *data) {
  assert(callback && data);
  assert(data[0] == 0 && data[1] == 255 && data[2] == 7 && data[3] == 0);
  return callback(data);
}
