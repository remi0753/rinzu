#ifndef RINZU_CALLBACK_CONSUMPTION_H
#define RINZU_CALLBACK_CONSUMPTION_H
#include "callback_return.h"

/* Private runtime protocol. An ID is never a pointer and is never reused. */
typedef struct rinzu_return_scope {
  uint64_t id;
  unsigned entered;
  rinzu_callback_return_node_t *nodes;
  rinzu_callback_return_stats_t totals;
  struct rinzu_return_scope *next;
} rinzu_return_scope_t;
typedef struct {
  size_t allocated, released, promoted;
  size_t allocated_bytes, released_bytes, promoted_bytes;
  size_t live_bytes, peak_bytes, copied_bytes, active_scopes, metadata_bytes;
} rinzu_return_scope_stats_t;
static rinzu_return_scope_t *rinzu_return_scopes;
static uint64_t rinzu_return_scope_sequence;
static rinzu_return_scope_stats_t rinzu_return_scope_totals;
static inline size_t rinzu_return_scope_add(size_t left, size_t right) {
  if (right > SIZE_MAX - left)
    _Exit(71);
  return left + right;
}
static inline rinzu_return_scope_t **rinzu_return_scope_find(uint64_t id) {
  rinzu_return_scope_t **slot = &rinzu_return_scopes;
  while (*slot && (*slot)->id != id)
    slot = &(*slot)->next;
  return slot;
}
static inline uint64_t rinzu_return_scope_prepare(void) {
  rinzu_return_scope_t *scope = calloc(1, sizeof(*scope));
  if (!scope)
    _Exit(71);
  rinzu_callback_return_lock();
  if (rinzu_return_scope_sequence == UINT64_MAX)
    _Exit(71);
  scope->id = ++rinzu_return_scope_sequence;
  scope->next = rinzu_return_scopes;
  rinzu_return_scopes = scope;
  rinzu_return_scope_stats_t *stats = &rinzu_return_scope_totals;
  stats->active_scopes = rinzu_return_scope_add(stats->active_scopes, 1);
  stats->metadata_bytes = rinzu_return_scope_add(stats->metadata_bytes, sizeof(*scope));
  uint64_t id = scope->id;
  rinzu_callback_return_unlock();
  return id;
}
static inline unsigned rinzu_return_scope_enter(uint64_t id) {
  rinzu_callback_return_lock();
  rinzu_return_scope_t *scope = *rinzu_return_scope_find(id);
  unsigned accepted = scope && !scope->entered;
  if (accepted)
    scope->entered = 1;
  rinzu_callback_return_unlock();
  return accepted;
}
static inline const unsigned char *rinzu_return_scope_copy(uint64_t id, const unsigned char *input,
                                                           size_t prefix, size_t length,
                                                           unsigned string, unsigned fallback) {
  rinzu_callback_return_node_t *node =
      rinzu_callback_return_allocate(input, prefix, length, string, fallback);
  rinzu_callback_return_lock();
  rinzu_return_scope_t *scope = *rinzu_return_scope_find(id);
  if (!scope)
    _Exit(86);
  rinzu_callback_return_add(&scope->totals, fallback, node->bytes);
  node->next = scope->nodes;
  scope->nodes = node;
  rinzu_return_scope_stats_t *stats = &rinzu_return_scope_totals;
  stats->allocated = rinzu_return_scope_add(stats->allocated, 1);
  stats->allocated_bytes = rinzu_return_scope_add(stats->allocated_bytes, node->bytes);
  stats->copied_bytes = rinzu_return_scope_add(stats->copied_bytes, prefix);
  stats->live_bytes = rinzu_return_scope_add(stats->live_bytes, node->bytes);
  stats->metadata_bytes = rinzu_return_scope_add(stats->metadata_bytes, sizeof(*node));
  if (stats->live_bytes > stats->peak_bytes)
    stats->peak_bytes = stats->live_bytes;
  const unsigned char *result = node->buffer;
  rinzu_callback_return_unlock();
  return result;
}
/* 0: unexposed abandonment; 1: proven physical consumption; 2: promotion;
   3: guard drop (prepared abandons; entered but unreceipted promotes).
   Mode 1 requires a closed consumer AND all physical uses/entries settled. */
static inline unsigned rinzu_return_scope_finish(uint64_t id, unsigned mode) {
  rinzu_callback_return_lock();
  rinzu_return_scope_t **slot = rinzu_return_scope_find(id);
  rinzu_return_scope_t *scope = *slot;
  if (!scope || mode > 3 || (mode == 0 && scope->entered) || (mode == 1 && !scope->entered)) {
    rinzu_callback_return_unlock();
    return 0;
  }
  unsigned promote = mode == 2 || (mode == 3 && scope->entered);
  *slot = scope->next; /* Retire the ID before release. Replayed IDs cannot affect a new scope. */
  rinzu_return_scope_stats_t *stats = &rinzu_return_scope_totals;
  size_t count = rinzu_return_scope_add(scope->totals.normal, scope->totals.fallback);
  size_t *out_count = promote ? &stats->promoted : &stats->released;
  size_t *out_bytes = promote ? &stats->promoted_bytes : &stats->released_bytes;
  *out_count = rinzu_return_scope_add(*out_count, count);
  *out_bytes = rinzu_return_scope_add(*out_bytes, scope->totals.bytes);
  if (stats->live_bytes < scope->totals.bytes || !stats->active_scopes ||
      stats->metadata_bytes < scope->totals.metadata_bytes + sizeof(*scope))
    _Exit(86);
  stats->live_bytes -= scope->totals.bytes;
  stats->metadata_bytes -= scope->totals.metadata_bytes + sizeof(*scope);
  --stats->active_scopes;
  if (promote) {
    rinzu_callback_return_stats_t *process = &rinzu_callback_return_totals;
    process->normal = rinzu_return_scope_add(process->normal, scope->totals.normal);
    process->fallback = rinzu_return_scope_add(process->fallback, scope->totals.fallback);
    process->bytes = rinzu_return_scope_add(process->bytes, scope->totals.bytes);
    process->metadata_bytes =
        rinzu_return_scope_add(process->metadata_bytes, scope->totals.metadata_bytes);
    process->normal_bytes =
        rinzu_return_scope_add(process->normal_bytes, scope->totals.normal_bytes);
    process->fallback_bytes =
        rinzu_return_scope_add(process->fallback_bytes, scope->totals.fallback_bytes);
  }
  rinzu_callback_return_node_t *node = scope->nodes;
  while (node) {
    rinzu_callback_return_node_t *next = node->next;
    if (promote) {
      node->next = rinzu_callback_return_nodes;
      rinzu_callback_return_nodes = node;
    } else {
      free(node->buffer);
      free(node);
    }
    node = next;
  }
  free(scope);
  rinzu_callback_return_unlock();
  return 1;
}
static inline rinzu_return_scope_stats_t rinzu_return_scope_stats(void) {
  rinzu_callback_return_lock();
  rinzu_return_scope_stats_t stats = rinzu_return_scope_totals;
  rinzu_callback_return_unlock();
  return stats;
}
typedef struct {
  rinzu_callback_return_stats_t process;
  rinzu_return_scope_stats_t scopes;
  size_t process_copied_bytes;
} rinzu_return_snapshot_t;
static inline rinzu_return_snapshot_t rinzu_return_snapshot(void) {
  rinzu_callback_return_lock();
  rinzu_return_snapshot_t result = {rinzu_callback_return_totals, rinzu_return_scope_totals,
                                    rinzu_callback_return_process_copied};
  rinzu_callback_return_unlock();
  return result;
}
#endif
