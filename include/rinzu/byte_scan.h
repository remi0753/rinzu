#ifndef RINZU_BYTE_SCAN_H
#define RINZU_BYTE_SCAN_H

#include <stddef.h>
#include <stdint.h>

static inline size_t rinzu_byte_find_set(const unsigned char *source, size_t length,
                                         const unsigned char *set, size_t count, size_t start,
                                         int in_set) {
  uint64_t members[4] = {0};
  for (size_t i = 0; i < count; ++i)
    members[set[i] >> 6] |= UINT64_C(1) << (set[i] & 63);
  for (size_t i = start; i < length; ++i) {
    int member = !!(members[source[i] >> 6] & (UINT64_C(1) << (source[i] & 63)));
    if (member == in_set)
      return i;
  }
  return SIZE_MAX;
}

#endif
