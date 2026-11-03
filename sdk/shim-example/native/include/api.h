#ifndef RINZU_RINZU_SHIM_13883CC0_H
#define RINZU_RINZU_SHIM_13883CC0_H
#include <stdint.h>
#ifdef __cplusplus
#define RINZU_SHIM_NOEXCEPT noexcept
extern "C" {
#else
#define RINZU_SHIM_NOEXCEPT
#endif
struct ValueResult {
  int32_t status;
  int64_t value;
};
void rinzu_rinzu_shim_13883cc0_calculate(int64_t value,
                                         struct ValueResult *output) RINZU_SHIM_NOEXCEPT;
#ifdef __cplusplus
}
#endif
#undef RINZU_SHIM_NOEXCEPT
#endif
