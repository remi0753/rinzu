#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <time.h>
int64_t rinzu_bench_nanoseconds(void) {
  struct timespec now;
  if (clock_gettime(CLOCK_MONOTONIC, &now))
    return -1;
  return now.tv_sec * INT64_C(1000000000) + now.tv_nsec;
}
bool rinzu_bench_verify(const uint8_t *data, size_t length, int64_t offset) {
  static const uint8_t pattern[] = {0, 255, 128, 65, 0, 66, 254, 67};
  for (size_t i = 0; i < length; ++i)
    if (data[i] != pattern[((size_t)offset + i) % sizeof(pattern)])
      return false;
  return true;
}
