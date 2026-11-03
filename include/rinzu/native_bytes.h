#ifndef RINZU_NATIVE_BYTES_H
#define RINZU_NATIVE_BYTES_H
#include <stddef.h>
#include <stdint.h>
/* Ordinary synchronous native ByteArray result. Return a pointer to this POD;
   both descriptor and data stay stable through managed result reception.
   The runtime copies length bytes, never adopts or frees the native buffer.
   NULL descriptor is a NULL result Error. length==0 permits NULL data.
   A positive length requires readable data; native faults are process-fatal.
   data remains native-owned and must not be modified during reception. */
typedef struct rinzu_native_byte_array {
  uint8_t *data;
  size_t length;
} rinzu_native_byte_array_t;
#endif
