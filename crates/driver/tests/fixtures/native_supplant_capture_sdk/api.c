#include "api.h"
int64_t capture_invoke(int64_t (*handler)(int64_t), int64_t value) {
  return handler(value);
}
