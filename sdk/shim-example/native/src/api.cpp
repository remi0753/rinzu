#include "api.h"
#include <limits.h>
#include <stdexcept>
#include <new>
#include <cstdlib>
#include <cstdio>

// These external shapes never cross the C-compatible header.
struct ExternalValue {
  union {
    int64_t integer;
    double decimal;
  } payload;
  unsigned integral : 1;
};
static ExternalValue external_value(int64_t value) {
  ExternalValue result{};
  result.integral = 1;
  result.payload.integer = value;
  return result; // External struct by value.
}
void rinzu_rinzu_shim_13883cc0_calculate(int64_t value, struct ValueResult *output) noexcept {
  try {
    if (value == -1)
      throw std::runtime_error("shim");
    if (value > INT64_MAX - 17) {
      output->status = -7604;
      output->value = 0;
      return;
    }
    const ExternalValue external = external_value(value);
    char text[32];
    // A fixed format and typed arguments contain the external varargs call.
    const int length =
        std::snprintf(text, sizeof(text), "%lld", (long long)external.payload.integer);
    if (!external.integral || length < 0 || (size_t)length >= sizeof(text)) {
      output->status = -7603;
      output->value = 0;
      return;
    }
    output->status = 0;
    output->value = external.payload.integer + 17;
  } catch (const std::bad_alloc &) {
    std::_Exit(70);
  } catch (...) {
    output->status = -7601;
    output->value = 0;
  }
}
