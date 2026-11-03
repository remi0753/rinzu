#ifndef RINZU_CALLBACK_RETURN_H
#define RINZU_CALLBACK_RETURN_H
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <pthread.h>
#include <unistd.h>

/* Nonmoving process data only. No managed handle, Owner, handler or code root. */
typedef struct rinzu_callback_return_node {
  unsigned char *buffer;
  size_t bytes;
  struct rinzu_callback_return_node *next;
} rinzu_callback_return_node_t;
typedef struct {
  size_t normal, fallback, bytes, metadata_bytes;
  size_t normal_bytes, fallback_bytes;
} rinzu_callback_return_stats_t;
static pthread_mutex_t rinzu_callback_return_mutex = PTHREAD_MUTEX_INITIALIZER;
static rinzu_callback_return_node_t *rinzu_callback_return_nodes;
static rinzu_callback_return_stats_t rinzu_callback_return_totals;
static size_t rinzu_callback_return_process_copied;
static inline void rinzu_callback_return_lock(void) {
  if (pthread_mutex_lock(&rinzu_callback_return_mutex))
    _Exit(86);
}
static inline void rinzu_callback_return_unlock(void) {
  if (pthread_mutex_unlock(&rinzu_callback_return_mutex))
    _Exit(86);
}
static inline rinzu_callback_return_node_t *
rinzu_callback_return_allocate(const unsigned char *input, size_t prefix, size_t length,
                               unsigned string, unsigned fallback) {
  if (prefix > length || (prefix && !input) || string > 1 || fallback > 1 ||
      (string && prefix != length))
    _Exit(86);
  if (length > PTRDIFF_MAX || (string && length == SIZE_MAX))
    _Exit(71);
  size_t bytes = length + string;
  if (!bytes)
    bytes = 1;
  rinzu_callback_return_node_t *node = malloc(sizeof(*node));
  if (!node)
    _Exit(71);
  unsigned char *buffer = calloc(bytes, 1);
  if (!buffer)
    _Exit(71);
  if (prefix)
    memcpy(buffer, input, prefix);
  *node = (rinzu_callback_return_node_t){buffer, bytes, NULL};
  return node;
}
/* Caller holds the arena mutex. Both process and scoped lists use this layout. */
static inline void rinzu_callback_return_add(rinzu_callback_return_stats_t *stats,
                                             unsigned fallback, size_t bytes) {
  size_t *count = fallback ? &stats->fallback : &stats->normal;
  size_t *payload = fallback ? &stats->fallback_bytes : &stats->normal_bytes;
  if (*count == SIZE_MAX || stats->bytes > SIZE_MAX - bytes ||
      stats->metadata_bytes > SIZE_MAX - sizeof(rinzu_callback_return_node_t) ||
      *payload > SIZE_MAX - bytes)
    _Exit(71);
  ++*count;
  stats->bytes += bytes;
  *payload += bytes;
  stats->metadata_bytes += sizeof(rinzu_callback_return_node_t);
}
static inline const unsigned char *rinzu_callback_return_buffer_copy(const unsigned char *input,
                                                                     size_t prefix, size_t length,
                                                                     unsigned string,
                                                                     unsigned fallback) {
  rinzu_callback_return_node_t *node =
      rinzu_callback_return_allocate(input, prefix, length, string, fallback);
  rinzu_callback_return_lock();
  rinzu_callback_return_add(&rinzu_callback_return_totals, fallback, node->bytes);
  if (prefix > SIZE_MAX - rinzu_callback_return_process_copied)
    _Exit(71);
  rinzu_callback_return_process_copied += prefix;
  node->next = rinzu_callback_return_nodes;
  rinzu_callback_return_nodes = node;
  rinzu_callback_return_unlock();
  return node->buffer;
}
static inline rinzu_callback_return_stats_t rinzu_callback_return_buffer_stats(void) {
  rinzu_callback_return_lock();
  rinzu_callback_return_stats_t stats = rinzu_callback_return_totals;
  rinzu_callback_return_unlock();
  return stats;
}
static inline size_t rinzu_callback_return_process_copy_bytes(void) {
  rinzu_callback_return_lock();
  size_t result = rinzu_callback_return_process_copied;
  rinzu_callback_return_unlock();
  return result;
}
#endif
