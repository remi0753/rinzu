#include <stdint.h>
#include <stdio.h>

extern int64_t rinzu_backend_spike(int64_t root, int64_t value);

void rinzu_backend_spike_safepoint(void) {}

int main(void) {
  int64_t result = rinzu_backend_spike(INT64_C(0x1000), INT64_C(41));
  if (result != 42) {
    fprintf(stderr, "AOT C ABI result: %lld\n", (long long)result);
    return 1;
  }
  return 0;
}
