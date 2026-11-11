#ifndef RINZU_NATIVE_READ_VIEW_H
#define RINZU_NATIVE_READ_VIEW_H

#include "rinzu/native_bytes.h"

/* A native consumer receives an opaque accessor and a generation from the
   provider. The accessor validates the generation and current thread. It
   returns NULL outside the provider's synchronous read scope. Consumers must
   treat the descriptor and bytes as read-only and use them within this call. */
typedef const rinzu_native_byte_array_t *(*rinzu_native_read_access_t)(void *view);

static inline const rinzu_native_byte_array_t *rinzu_native_read_view(void *access, void *view) {
  return access ? ((rinzu_native_read_access_t)access)(view) : NULL;
}

#endif
