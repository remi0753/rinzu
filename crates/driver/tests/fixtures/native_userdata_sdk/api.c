#include "api.h"
#include <stddef.h>
static _Thread_local void *previous[5];
static _Thread_local uint64_t entries;
int32_t userdata_probe(void *first, void *same, void *second, void *event, void *other,
                       uint64_t (*callback)(const uint8_t *), int32_t mode) {
  void *actual[] = {first, same, second, event, other};
  for (size_t i = 0; i < 5; ++i) {
    if (!actual[i])
      return 11;
    for (size_t j = 0; j < 5; ++j) {
      if (actual[i] == previous[j] || (i != j && actual[i] == actual[j]))
        return 12;
    }
  }
  ++entries;
  uint8_t input[] = {0, 255, 7, 0};
  for (size_t i = 0; i < 32; ++i) {
    if (callback(input) != 4)
      return 13;
    input[2] = (uint8_t)i;
  }
  for (size_t i = 0; i < 5; ++i)
    previous[i] = actual[i];
  return mode ? 17 : 0;
}
uint64_t userdata_entries(void) {
  return entries;
}
