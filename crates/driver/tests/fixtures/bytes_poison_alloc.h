#include <stdlib.h>
#include <string.h>

/* Leave the common calloc metadata zeroed. */
static inline void *bytes_poison_malloc(size_t size) {
  void *memory = malloc(size);
  if (memory)
    memset(memory, 165, size);
  return memory;
}

#define malloc bytes_poison_malloc
