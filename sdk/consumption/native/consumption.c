#include "consumption.h"
static __attribute__((noinline)) uint64_t read_string(const char *data) {
  uint64_t sum = 0;
  for (uint64_t index = 0; data[index]; ++index)
    sum += (unsigned char)data[index];
  return sum;
}
static __attribute__((noinline)) uint64_t read_four(const unsigned char *data) {
  uint64_t sum = 0;
  for (uint64_t index = 0; index < 4; ++index)
    sum += data[index];
  return sum;
}
uint64_t rinzu_consume_string(const char *(*producer)(uint64_t), uint64_t count) {
  uint64_t sum = 0;
  for (uint64_t index = 0; index < count; ++index) {
    const char *data = producer(index);
    sum += read_string(data);
    sum += read_string(data);
  }
  return sum;
}
uint64_t rinzu_consume_bytes4(const unsigned char *(*producer)(uint64_t), uint64_t count) {
  uint64_t sum = 0;
  for (uint64_t index = 0; index < count; ++index) {
    const unsigned char *data = producer(index);
    sum += read_four(data);
    sum += read_four(data);
  }
  return sum;
}
