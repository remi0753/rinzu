#ifndef RINZU_KEY_H
#define RINZU_KEY_H
#include "rinzu/scalar_transport.h"

/* Private content hash. The caller supplies verified typed values, never
   managed addresses. A hash collision must still use value equality. */
typedef struct {
  uint64_t hash;
} rinzu_key_hasher_t;
/* Validation and scalar count share one pass. SIZE_MAX is the invalid sentinel;
   managed payload lengths exclude it, including for an empty String. */
static inline size_t rinzu_utf8_scalar_length(const unsigned char *bytes, size_t length) {
  if (length == SIZE_MAX || (length && !bytes))
    return SIZE_MAX;
  size_t scalars = 0;
  for (size_t index = 0; index < length;) {
    unsigned first = bytes[index++];
    scalars++;
    if (first < 0x80)
      continue;
    size_t tail =
        first >= 0xc2 && first <= 0xdf
            ? 1
            : (first >= 0xe0 && first <= 0xef ? 2 : (first >= 0xf0 && first <= 0xf4 ? 3 : 0));
    if (!tail || tail > length - index)
      return SIZE_MAX;
    unsigned second = bytes[index];
    if ((first == 0xe0 && second < 0xa0) || (first == 0xed && second >= 0xa0) ||
        (first == 0xf0 && second < 0x90) || (first == 0xf4 && second >= 0x90))
      return SIZE_MAX;
    while (tail--) {
      if (bytes[index] < 0x80 || bytes[index] > 0xbf)
        return SIZE_MAX;
      index += 1;
    }
  }
  return scalars;
}
static inline int rinzu_utf8_valid(const unsigned char *bytes, size_t length) {
  return rinzu_utf8_scalar_length(bytes, length) != SIZE_MAX;
}
static inline rinzu_key_hasher_t rinzu_key_init(void) {
  return (rinzu_key_hasher_t){UINT64_C(0xcbf29ce484222325)};
}
static inline void rinzu_key_write(rinzu_key_hasher_t *hash, const unsigned char *bytes,
                                   size_t length) {
  for (size_t index = 0; index < length; ++index)
    hash->hash = (hash->hash ^ bytes[index]) * UINT64_C(0x100000001b3);
}
static inline void rinzu_key_u64(rinzu_key_hasher_t *hash, uint64_t bits, size_t width) {
  unsigned char bytes[8];
  for (size_t index = 0; index < width; ++index)
    bytes[index] = (unsigned char)(bits >> (index * 8));
  rinzu_key_write(hash, bytes, width);
}
static inline int rinzu_key_integer(rinzu_key_hasher_t *hash, unsigned kind, uint64_t bits) {
  if (kind > RINZU_SCALAR_U64 || !rinzu_transfer_valid(kind, bits))
    return RINZU_TRANSFER_ENCODING;
  unsigned char prefix[] = {0x10, (unsigned char)kind};
  rinzu_key_write(hash, prefix, sizeof(prefix));
  rinzu_key_u64(hash, bits, rinzu_scalar_width(kind) / 8);
  return RINZU_TRANSFER_OK;
}
static inline int rinzu_key_bool(rinzu_key_hasher_t *hash, uint64_t bits) {
  if (bits > 1)
    return RINZU_TRANSFER_ENCODING;
  const unsigned char prefix[] = {0x11, (unsigned char)bits};
  rinzu_key_write(hash, prefix, sizeof(prefix));
  return RINZU_TRANSFER_OK;
}
static inline int rinzu_key_bytes(rinzu_key_hasher_t *hash, const unsigned char *bytes,
                                  size_t length) {
  if (length && !bytes)
    return RINZU_TRANSFER_LENGTH;
  const unsigned char prefix = 0x21;
  rinzu_key_write(hash, &prefix, 1);
  rinzu_key_u64(hash, length, 8);
  rinzu_key_write(hash, bytes, length);
  return RINZU_TRANSFER_OK;
}
static inline int rinzu_key_string(rinzu_key_hasher_t *hash, const unsigned char *bytes,
                                   size_t length) {
  if (length && !bytes)
    return RINZU_TRANSFER_LENGTH;
  if (!rinzu_utf8_valid(bytes, length))
    return RINZU_TRANSFER_ENCODING;
  const unsigned char prefix = 0x20;
  rinzu_key_write(hash, &prefix, 1);
  rinzu_key_u64(hash, length, 8);
  rinzu_key_write(hash, bytes, length);
  return RINZU_TRANSFER_OK;
}
#endif
