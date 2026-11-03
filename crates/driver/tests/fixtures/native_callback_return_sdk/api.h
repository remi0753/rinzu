#include <stdint.h>
#include <stddef.h>
uint64_t return_strings(const char *(*)(int32_t, const char *), int32_t);
uint64_t return_bytes(const uint8_t *(*)(int32_t, const char *), int32_t);
int32_t return_zero(const uint8_t *(*)(int32_t));
int32_t return_empty(const char *(*)(int32_t));
int32_t return_unit(void (*)(int32_t));
void return_tick(void);
int32_t return_held(void);
