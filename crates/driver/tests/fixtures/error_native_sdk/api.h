#include <stdint.h>
int64_t error_apply(int64_t (*callback)(int64_t), int64_t value);
int64_t error_input(int64_t (*callback)(const char *), int64_t mode);
