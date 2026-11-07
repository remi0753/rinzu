/* Measure the update's allocations independently of metadata/enum reuse.
   The same controls also keep each OOM probe tied to a physical allocation. */
#define RINZU_MANAGED_METADATA_CACHE_CAPACITY 0
#define RINZU_MANAGED_SMALL_ENUM_MODE 0
#include <stdlib.h>
#include <string.h>

static size_t list_allocations, list_copy_bytes, list_fail_at;
static int list_count_active;

static inline void *list_calloc(size_t n, size_t size) {
  if (list_count_active && ++list_allocations == list_fail_at)
    return NULL;
  return calloc(n, size);
}
static inline void *list_memcpy(void *out, const void *in, size_t n) {
  if (list_count_active)
    list_copy_bytes += n;
  return memcpy(out, in, n);
}
static inline void *list_memmove(void *out, const void *in, size_t n) {
  if (list_count_active)
    list_copy_bytes += n;
  return memmove(out, in, n);
}

#undef memcpy
#undef memmove
#define calloc list_calloc
#define memcpy list_memcpy
#define memmove list_memmove
