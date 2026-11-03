#include "api.h"
int64_t error_apply(int64_t (*callback)(int64_t), int64_t value) {
  return callback(value);
}
int64_t error_input(int64_t (*callback)(const char *), int64_t mode) {
  static const char invalid[] = {(char)0xff, 0};
  return callback(mode == 0 ? "text" : mode == 1 ? 0 : invalid);
}
