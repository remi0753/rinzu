#include "api.h"
static int64_t handle = 5;
void *context_make(void) {
  return &handle;
}
int64_t context_check(void *value) {
  return value == &handle ? handle : -100;
}
int64_t context_change(const struct Packet *value, int64_t amount) {
  ((struct Packet *)value)->x += amount;
  return 0;
}
int64_t context_invoke(void *value, int64_t (*handler)(void *)) {
  return handler(value);
}
