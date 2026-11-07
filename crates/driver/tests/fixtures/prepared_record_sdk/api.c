#include "api.h"
#include <string.h>
static _Thread_local struct Wide wide;
static _Thread_local struct Single single;
static _Thread_local struct Mixed mixed;
const struct Wide *prepared_wide(int64_t seed, int64_t (*callback)(int64_t)) {
  if (seed < 0)
    return NULL;
  wide.left = callback(seed);
  wide.right = UINT64_MAX;
  return &wide;
}
const struct Single *prepared_single(int64_t value) {
  single.value = value;
  return &single;
}
const struct Mixed *prepared_mixed(int64_t kind) {
  memset(&mixed, 0, sizeof(mixed));
  unsigned char flag = kind == 2 ? 2 : 1;
  memcpy(&mixed.flag, &flag, 1);
  mixed.count = 7;
  return &mixed;
}
void prepared_mutate(void) {
  memset(&wide, 0, sizeof(wide));
  memset(&single, 0, sizeof(single));
  memset(&mixed, 0, sizeof(mixed));
}
