#ifndef RINZU_MANAGED_ROOTS_H
#define RINZU_MANAGED_ROOTS_H
#include "rinzu/failure.h"
#include "rinzu/resource_counters.h"
#include <string.h>

typedef struct rinzu_managed_frame {
  uint64_t *roots;
  size_t count;
  size_t capacity;
  size_t prepared_count;
  /* 0: chain-local heap frame; 1: embedded admission frame;
     2: Domain-cache heap frame; 3: inactive, retained bytes accounted. */
  unsigned writing, allocation;
  struct rinzu_managed_frame *previous;
} rinzu_managed_frame_t;
#ifndef RINZU_MANAGED_FRAME_CACHE_COUNT
#define RINZU_MANAGED_FRAME_CACHE_COUNT 64
#endif
#ifndef RINZU_MANAGED_FRAME_CACHE_BYTES
#define RINZU_MANAGED_FRAME_CACHE_BYTES (64 * 1024)
#endif
typedef struct {
  rinzu_managed_frame_t *frames;
  size_t count, bytes, borrowed;
} rinzu_managed_frame_cache_t;
static __attribute__((noinline, cold)) void
rinzu_managed_frame_cache_charge(size_t bytes) {
  rinzu_resource_acquire(RINZU_RESOURCE_RUNTIME_CONTROL, bytes);
}
static __attribute__((noinline, cold)) void
rinzu_managed_frame_cache_uncharge(size_t bytes) {
  rinzu_resource_release(RINZU_RESOURCE_RUNTIME_CONTROL, bytes);
}
static inline size_t rinzu_managed_frame_bytes(const rinzu_managed_frame_t *frame) {
  if (frame->capacity > (SIZE_MAX - sizeof(*frame)) / sizeof(uint64_t))
    rinzu_failure_fatal();
  return sizeof(*frame) + frame->capacity * sizeof(uint64_t);
}
/* Only the Owner changes registered chains and this Domain-local allocator.
   Inactive slots are not roots. No native/callback/GC transition occurs here. */
static inline rinzu_managed_frame_t *
rinzu_managed_frame_cache_take(rinzu_managed_frame_cache_t *cache) {
  if (cache->borrowed == SIZE_MAX) rinzu_failure_fatal();
  rinzu_managed_frame_t *frame = cache->frames;
  if (frame) {
    size_t bytes = rinzu_managed_frame_bytes(frame);
    if (!cache->count || cache->bytes < bytes || frame->count || frame->writing ||
        (frame->allocation != 2 && frame->allocation != 3))
      rinzu_failure_fatal();
    cache->frames = frame->previous;
    --cache->count;
    cache->bytes -= bytes;
    if (frame->allocation == 3) rinzu_managed_frame_cache_uncharge(bytes);
  } else {
    frame = rinzu_failure_allocate(sizeof(*frame));
  }
  frame->allocation = 2;
  ++cache->borrowed;
  return frame;
}
static inline void rinzu_managed_frame_cache_put(rinzu_managed_frame_cache_t *cache,
                                                 rinzu_managed_frame_t *frame) {
  if (!cache->borrowed || frame->count || frame->writing || frame->allocation != 2)
    rinzu_failure_fatal();
  --cache->borrowed;
  size_t bytes = rinzu_managed_frame_bytes(frame);
  if (cache->count < RINZU_MANAGED_FRAME_CACHE_COUNT &&
      bytes <= RINZU_MANAGED_FRAME_CACHE_BYTES &&
      cache->bytes <= RINZU_MANAGED_FRAME_CACHE_BYTES - bytes) {
    if (atomic_load_explicit(&rinzu_resource_enabled, memory_order_relaxed)) {
      rinzu_managed_frame_cache_charge(bytes);
      frame->allocation = 3;
    }
    frame->previous = cache->frames;
    cache->frames = frame;
    ++cache->count;
    cache->bytes += bytes;
  } else {
    free(frame->roots);
    free(frame);
  }
}
static inline void rinzu_managed_frame_cache_dispose(rinzu_managed_frame_cache_t *cache) {
  if (cache->borrowed) rinzu_failure_fatal();
  while (cache->frames) {
    rinzu_managed_frame_t *frame = cache->frames;
    size_t bytes = rinzu_managed_frame_bytes(frame);
    if (!cache->count || cache->bytes < bytes || frame->count || frame->writing ||
        (frame->allocation != 2 && frame->allocation != 3))
      rinzu_failure_fatal();
    cache->frames = frame->previous;
    --cache->count;
    cache->bytes -= bytes;
    if (frame->allocation == 3) rinzu_managed_frame_cache_uncharge(bytes);
    free(frame->roots);
    free(frame);
  }
  if (cache->count || cache->bytes) rinzu_failure_fatal();
}
typedef struct {
  /* A frame chain may be registered once in one Domain. This is native
     metadata, not a managed root and not a scanned continuation address. */
  struct rinzu_managed_root_set *domain_entry;
  size_t *domain_published_count;
  rinzu_managed_frame_cache_t *frame_cache;
  rinzu_managed_frame_t *current;
  rinzu_managed_frame_t *free_frames;
  uint64_t depth, peak_depth, publications;
} rinzu_managed_roots_t;
/* Counts are diagnostic metadata, never a replacement for tracing frame slots.
   Only the Owner changes a registered frame chain. Equal-size publications
   don't change this derived value. No calls or allocation occur here. */
static inline void rinzu_managed_set_count(rinzu_managed_roots_t *state, size_t count) {
  rinzu_managed_frame_t *frame = state->current;
  if (!frame || count > frame->capacity || frame->count > frame->capacity)
    rinzu_failure_fatal();
  if (state->domain_published_count && count != frame->count) {
    size_t total = *state->domain_published_count;
    if (total < frame->count)
      rinzu_failure_fatal();
    total -= frame->count;
    if (count > SIZE_MAX - total)
      rinzu_resource_fatal();
    *state->domain_published_count = total + count;
  }
  frame->count = count;
}
static inline size_t rinzu_managed_frame_chain_count(const rinzu_managed_roots_t *state) {
  size_t count = 0;
  for (const rinzu_managed_frame_t *frame = state->current; frame; frame = frame->previous) {
    if (frame->count > SIZE_MAX - count)
      rinzu_resource_fatal();
    count += frame->count;
  }
  return count;
}
static inline void rinzu_managed_enter(rinzu_managed_roots_t *state) {
  if (state->current && state->current->writing)
    rinzu_failure_fatal();
  rinzu_managed_frame_t *frame = state->free_frames;
  if (frame)
    state->free_frames = frame->previous;
  else if (state->frame_cache)
    frame = rinzu_managed_frame_cache_take(state->frame_cache);
  else
    frame = rinzu_failure_allocate(sizeof(*frame));
  frame->count = 0;
  frame->previous = state->current;
  state->current = frame;
  state->depth += 1;
  if (state->depth > state->peak_depth)
    state->peak_depth = state->depth;
}
/* Keep storage until Domain/task disposal. Only count entries are GC roots;
   inactive frames and unused capacity never keep managed objects alive. */
static inline uint64_t *rinzu_managed_reserve_roots(rinzu_managed_roots_t *state, size_t count) {
  rinzu_managed_frame_t *frame = state->current;
  if (!frame || frame->writing || count > SIZE_MAX / sizeof(uint64_t) ||
      frame->count > frame->capacity)
    rinzu_failure_fatal();
  if (count <= frame->capacity)
    return frame->roots;
  size_t capacity = frame->capacity ? frame->capacity : 8;
  while (capacity < count) {
    if (capacity > SIZE_MAX / sizeof(uint64_t) / 2) {
      capacity = count;
      break;
    }
    capacity *= 2;
  }
  uint64_t *roots = rinzu_failure_allocate(capacity * sizeof(*roots));
  if (frame->count)
    memcpy(roots, frame->roots, frame->count * sizeof(*roots));
  free(frame->roots);
  frame->roots = roots;
  frame->capacity = capacity;
  return roots;
}
/* Private compiler protocol: between prepare and commit there may only be
   stores into this buffer. No calls, allocation, yield or GC may occur. */
static inline uint64_t *rinzu_managed_prepare_roots(rinzu_managed_roots_t *state, int64_t count) {
  if (count < 0 || (uint64_t)count > SIZE_MAX / sizeof(uint64_t))
    rinzu_failure_fatal();
  uint64_t *roots = rinzu_managed_reserve_roots(state, (size_t)count);
  state->current->prepared_count = (size_t)count;
  state->current->writing = 1;
  return roots;
}
static inline void rinzu_managed_commit_roots(rinzu_managed_roots_t *state, int64_t count) {
  rinzu_managed_frame_t *frame = state->current;
  if (!frame || !frame->writing || count < 0 || (uint64_t)count != frame->prepared_count)
    rinzu_failure_fatal();
  rinzu_managed_set_count(state, frame->prepared_count);
  frame->writing = 0;
  state->publications += 1;
}
static inline void rinzu_managed_publish(rinzu_managed_roots_t *state, const uint64_t *roots,
                                         int64_t count) {
  if (!count) {
    rinzu_managed_frame_t *frame = state->current;
    if (!frame || frame->writing || frame->count > frame->capacity)
      rinzu_failure_fatal();
    rinzu_managed_set_count(state, 0);
    state->publications += 1;
    return;
  }
  if (!state->current || count < 0 || ((uint64_t)count > SIZE_MAX / sizeof(uint64_t)) ||
      (count && !roots))
    rinzu_failure_fatal();
  uint64_t *copy = rinzu_managed_reserve_roots(state, (size_t)count);
  if (count)
    memmove(copy, roots, (size_t)count * sizeof(*copy));
  rinzu_managed_set_count(state, (size_t)count);
  state->publications += 1;
}
static inline void rinzu_managed_leave(rinzu_managed_roots_t *state) {
  rinzu_managed_frame_t *frame = state->current;
  if (!frame || frame->writing || !state->depth)
    rinzu_failure_fatal();
  rinzu_managed_set_count(state, 0);
  state->current = frame->previous;
  state->depth -= 1;
  frame->count = 0;
  if (frame->allocation == 2) {
    if (!state->frame_cache) rinzu_failure_fatal();
    rinzu_managed_frame_cache_put(state->frame_cache, frame);
  } else {
    if (frame->allocation > 1) rinzu_failure_fatal();
    frame->previous = state->free_frames;
    state->free_frames = frame;
  }
}
static inline void rinzu_managed_dispose(rinzu_managed_roots_t *state) {
  if (state->current || state->depth || state->domain_entry || state->domain_published_count ||
      state->frame_cache)
    rinzu_failure_fatal();
  rinzu_managed_frame_t *frame = state->free_frames;
  while (frame) {
    if (frame->allocation) rinzu_failure_fatal();
    rinzu_managed_frame_t *next = frame->previous;
    free(frame->roots);
    free(frame);
    frame = next;
  }
  *state = (rinzu_managed_roots_t){0};
}
#endif
