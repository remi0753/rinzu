#ifndef RINZU_IDENTITY_INDEX_H
#define RINZU_IDENTITY_INDEX_H
#include <stddef.h>
#include <stdint.h>
/* Private Owner-only metadata indexes; capacity is a checked power of two.
   Index entries are borrowed pointers, never managed roots or reusable IDs. */
static inline size_t rinzu_identity_bucket(uint64_t identity, size_t capacity) {
  identity = (identity ^ (identity >> 30)) * UINT64_C(0xbf58476d1ce4e5b9);
  identity = (identity ^ (identity >> 27)) * UINT64_C(0x94d049bb133111eb);
  identity ^= identity >> 31;
  return (size_t)(identity & (capacity - 1));
}
#endif
