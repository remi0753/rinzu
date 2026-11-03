#ifndef RINZU_SCALAR_TRANSPORT_H
#define RINZU_SCALAR_TRANSPORT_H
#include "rinzu/scalar.h"
#include <stddef.h>

/* Private cross-Domain encoding, independent of native C parameter layouts.
   The verified function signature supplies kinds; the buffer contains only
   exact-width little-endian payloads. Unit contributes zero bytes. */
enum { RINZU_TRANSFER_UNIT = 11 };
enum { RINZU_TRANSFER_OK, RINZU_TRANSFER_KIND, RINZU_TRANSFER_ENCODING, RINZU_TRANSFER_LENGTH };

static inline size_t rinzu_transfer_width(unsigned kind) {
  return kind == RINZU_TRANSFER_UNIT
             ? 0
             : (kind <= RINZU_SCALAR_BOOL ? rinzu_scalar_width(kind) / 8 : SIZE_MAX);
}
static inline uint64_t rinzu_transfer_canonical(unsigned kind, uint64_t bits) {
  unsigned width = rinzu_scalar_width(kind);
  if (rinzu_scalar_signed(kind) && width < 64) {
    uint64_t mask = (UINT64_C(1) << width) - 1;
    return bits & (UINT64_C(1) << (width - 1)) ? bits | ~mask : bits;
  }
  return bits;
}
static inline int rinzu_transfer_valid(unsigned kind, uint64_t bits) {
  if (kind == RINZU_TRANSFER_UNIT)
    return bits == 0;
  if (kind > RINZU_SCALAR_BOOL)
    return 0;
  if (kind == RINZU_SCALAR_BOOL)
    return bits <= 1;
  unsigned width = rinzu_scalar_width(kind);
  if (width == 64)
    return 1;
  uint64_t mask = (UINT64_C(1) << width) - 1;
  return rinzu_transfer_canonical(kind, bits & mask) == bits;
}
static inline int rinzu_transfer_encode_scalar(unsigned kind, uint64_t bits, unsigned char *bytes,
                                               size_t length) {
  size_t width = rinzu_transfer_width(kind);
  if (width == SIZE_MAX)
    return RINZU_TRANSFER_KIND;
  if (length != width || (length && !bytes))
    return RINZU_TRANSFER_LENGTH;
  if (!rinzu_transfer_valid(kind, bits))
    return RINZU_TRANSFER_ENCODING;
  for (size_t index = 0; index < length; ++index)
    bytes[index] = (unsigned char)(bits >> (index * 8));
  return RINZU_TRANSFER_OK;
}
static inline rinzu_scalar_result_t
rinzu_transfer_decode_scalar(unsigned kind, const unsigned char *bytes, size_t length) {
  rinzu_scalar_result_t result = {0, RINZU_TRANSFER_OK};
  size_t width = rinzu_transfer_width(kind);
  if (width == SIZE_MAX) {
    result.status = RINZU_TRANSFER_KIND;
    return result;
  }
  if (length != width || (length && !bytes)) {
    result.status = RINZU_TRANSFER_LENGTH;
    return result;
  }
  for (size_t index = 0; index < length; ++index)
    result.bits |= (uint64_t)bytes[index] << (index * 8);
  result.bits = rinzu_transfer_canonical(kind, result.bits);
  if (!rinzu_transfer_valid(kind, result.bits))
    result.status = RINZU_TRANSFER_ENCODING;
  return result;
}
static inline int rinzu_transfer_record_size(const unsigned *kinds, size_t count, size_t *size) {
  if (!size || (count && !kinds))
    return RINZU_TRANSFER_LENGTH;
  size_t total = 0;
  for (size_t index = 0; index < count; ++index) {
    size_t width = rinzu_transfer_width(kinds[index]);
    if (width == SIZE_MAX)
      return RINZU_TRANSFER_KIND;
    if (width > SIZE_MAX - total)
      return RINZU_TRANSFER_LENGTH;
    total += width;
  }
  *size = total;
  return RINZU_TRANSFER_OK;
}
static inline int rinzu_transfer_encode_record(const unsigned *kinds, const uint64_t *slots,
                                               size_t count, unsigned char *bytes, size_t length) {
  size_t size = 0;
  int status = rinzu_transfer_record_size(kinds, count, &size);
  if (status)
    return status;
  if (size != length || (length && !bytes) || (count && !slots))
    return RINZU_TRANSFER_LENGTH;
  for (size_t index = 0; index < count; ++index)
    if (!rinzu_transfer_valid(kinds[index], slots[index]))
      return RINZU_TRANSFER_ENCODING;
  size_t offset = 0;
  for (size_t index = 0; index < count; ++index) {
    size_t width = rinzu_transfer_width(kinds[index]);
    status = rinzu_transfer_encode_scalar(kinds[index], slots[index], width ? bytes + offset : NULL,
                                          width);
    if (status)
      return status;
    offset += width;
  }
  return RINZU_TRANSFER_OK;
}
static inline int rinzu_transfer_decode_record(const unsigned *kinds, size_t count,
                                               const unsigned char *bytes, size_t length,
                                               uint64_t *slots) {
  size_t size = 0;
  int status = rinzu_transfer_record_size(kinds, count, &size);
  if (status)
    return status;
  if (size != length || (length && !bytes) || (count && !slots))
    return RINZU_TRANSFER_LENGTH;
  /* Validate all parameters before publishing even one destination slot. */
  for (unsigned pass = 0; pass < 2; ++pass) {
    size_t offset = 0;
    for (size_t index = 0; index < count; ++index) {
      size_t width = rinzu_transfer_width(kinds[index]);
      rinzu_scalar_result_t result =
          rinzu_transfer_decode_scalar(kinds[index], width ? bytes + offset : NULL, width);
      if (result.status)
        return result.status;
      if (pass)
        slots[index] = result.bits;
      offset += width;
    }
  }
  return RINZU_TRANSFER_OK;
}
#endif
