#ifndef RINZU_DISPLAY_H
#define RINZU_DISPLAY_H
#include <stddef.h>
#include <stdint.h>

/* Private display catalog, independent of transport/equality and native ABI. */
enum rinzu_display_kind {
  RINZU_DISPLAY_SCALAR,
  RINZU_DISPLAY_UNIT,
  RINZU_DISPLAY_STRING,
  RINZU_DISPLAY_BYTES,
  RINZU_DISPLAY_RECORD,
  RINZU_DISPLAY_ENUM,
  RINZU_DISPLAY_LIST,
  RINZU_DISPLAY_MAP,
  RINZU_DISPLAY_MARKER,
  RINZU_DISPLAY_ERROR
};
typedef struct {
  const unsigned char *bytes;
  size_t length;
} rinzu_display_text_t;
typedef struct {
  rinzu_display_text_t name;
  size_t type;
} rinzu_display_field_t;
typedef struct {
  rinzu_display_text_t name;
  const rinzu_display_field_t *fields;
  size_t count;
} rinzu_display_variant_t;
typedef struct {
  unsigned kind, scalar;
  rinzu_display_text_t name;
  const rinzu_display_field_t *fields;
  size_t count;
  const rinzu_display_variant_t *variants;
  size_t element, key, value;
} rinzu_display_type_t;
typedef struct {
  uint64_t bits;
  unsigned reference;
} rinzu_display_slot_t;
typedef struct {
  rinzu_display_text_t text;
  const rinzu_display_slot_t *fields;
  size_t count, variant;
} rinzu_display_node_t;
/* A shallow read on the Owner thread. Returned slices last until the next read.
 * Formatting makes no managed allocations, invokes no user code, and does not
 * enter native-safe state until the complete native byte buffer is ready. */
typedef int (*rinzu_display_load_t)(void *, size_t, uint64_t, rinzu_display_node_t *);
int rinzu_display_format(const rinzu_display_type_t *, size_t, size_t, rinzu_display_slot_t, int,
                         rinzu_display_load_t, void *, unsigned char **, size_t *);
#endif
