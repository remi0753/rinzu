#ifndef RINZU_NATIVE_OPAQUE_H
#define RINZU_NATIVE_OPAQUE_H
#include "rinzu/managed_data.h"

/* Private uninitialized reservation, never a source value. The closed call plan
   supplies its nominal index; each Domain validates handles in its own heap. */
typedef struct {
  uint64_t identity;
  size_t type;
} rinzu_native_opaque_reservation_t;
enum { RINZU_NATIVE_OPAQUE_OK = 0, RINZU_NATIVE_NULL_OPAQUE = 1 };
static inline rinzu_native_opaque_reservation_t
rinzu_managed_reserve_opaque(rinzu_managed_data_t *data, size_t type) {
  rinzu_managed_native_owner(data);
  rinzu_managed_value_t *value = rinzu_managed_new(data, RINZU_MANAGED_RESERVED_OPAQUE);
  value->type = type;
  return (rinzu_native_opaque_reservation_t){value->identity, type};
}
static inline void rinzu_managed_discard_opaque(rinzu_managed_data_t *data,
                                                rinzu_native_opaque_reservation_t *reservation) {
  if (!reservation || !reservation->identity)
    rinzu_interop_fatal();
  rinzu_managed_native_owner(data);
  rinzu_managed_value_t **cursor = &data->values;
  while (*cursor && (*cursor)->identity != reservation->identity)
    cursor = &(*cursor)->next;
  if (!*cursor || (*cursor)->kind != RINZU_MANAGED_RESERVED_OPAQUE ||
      (*cursor)->type != reservation->type || (*cursor)->native_pointer)
    rinzu_interop_fatal();
  rinzu_managed_value_t *value = *cursor;
  *cursor = value->next;
  rinzu_managed_index_remove(data, value);
  rinzu_managed_value_release(data, value);
  data->objects -= 1;
  /* External-return settlement must not allocate. Shrink live heaps only at
     collection; an empty index can be released without allocating. */
  if (!data->objects)
    rinzu_managed_index_trim(data);
  reservation->identity = 0;
}
/* No allocation, target memory access, retain/release or address deduplication
   occurs after external return. Keep the existing reservation root until the
   initialized result has been handed to the continuation's root slot. */
static inline int rinzu_managed_receive_opaque(rinzu_managed_data_t *data,
                                               rinzu_native_opaque_reservation_t *reservation,
                                               const void *pointer, uint64_t *output) {
  if (!reservation || !reservation->identity || !output)
    rinzu_interop_fatal();
  rinzu_managed_native_owner(data);
  rinzu_managed_value_t *value = rinzu_managed_lookup(data, reservation->identity);
  if (!value || value->kind != RINZU_MANAGED_RESERVED_OPAQUE || value->type != reservation->type ||
      value->native_pointer)
    rinzu_interop_fatal();
  if (!pointer) {
    rinzu_managed_discard_opaque(data, reservation);
    return RINZU_NATIVE_NULL_OPAQUE;
  }
  value->native_pointer = (uintptr_t)pointer;
  value->kind = RINZU_MANAGED_NATIVE_OPAQUE;
  *output = value->identity;
  reservation->identity = 0;
  return RINZU_NATIVE_OPAQUE_OK;
}
static inline void *rinzu_managed_opaque_pointer(rinzu_managed_data_t *data, uint64_t identity,
                                                 size_t type) {
  rinzu_managed_native_owner(data);
  rinzu_managed_value_t *value = rinzu_managed_lookup(data, identity);
  if (!value || value->kind != RINZU_MANAGED_NATIVE_OPAQUE || value->type != type || !value->native_pointer)
    rinzu_interop_fatal();
  return (void *)value->native_pointer;
}
#endif
