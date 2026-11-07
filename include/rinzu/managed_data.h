#ifndef RINZU_MANAGED_DATA_H
#define RINZU_MANAGED_DATA_H
#include "rinzu/managed_roots.h"
#include "rinzu/value.h"
#include "rinzu/resource_counters.h"
#include "rinzu/gc_policy.h"
#include "rinzu/thread_identity.h"
#include <stdatomic.h>

#define RINZU_MANAGED_HANDLE_BASE (UINT64_C(1) << 62)
#define RINZU_MANAGED_FUTURE_BASE (UINT64_C(1) << 61)
static _Atomic uint64_t rinzu_managed_next_identity = RINZU_MANAGED_HANDLE_BASE;
enum rinzu_managed_kind {
  RINZU_MANAGED_STRING = 1,
  RINZU_MANAGED_ERROR = 2,
  RINZU_MANAGED_BYTES = 3,
  RINZU_MANAGED_COMPOSITE = 4,
  RINZU_MANAGED_RESERVED_OPAQUE = 5,
  RINZU_MANAGED_NATIVE_OPAQUE = 6,
  RINZU_MANAGED_RICH_ERROR = 7
};
/* Reference tags come from typed frames. Full-width integers may have exactly
   the same bits as a handle and must never be inferred to be references. */
typedef struct {
  uint64_t bits;
  unsigned reference;
} rinzu_managed_slot_t;
/* Immutable Map indexes contain slot offsets, never pointers into a movable
   payload. Entries retain declaration order; zero marks an empty bucket. */
typedef struct {
  uint64_t hash;
  size_t slot_plus_one;
} rinzu_managed_map_bucket_t;
typedef struct {
  size_t capacity;
  rinzu_managed_map_bucket_t buckets[];
} rinzu_managed_map_index_t;
static inline size_t rinzu_managed_map_index_bytes(const rinzu_managed_map_index_t *index) {
  return index ? sizeof(*index) + index->capacity * sizeof(*index->buckets) : 0;
}
typedef struct rinzu_managed_value {
  uint64_t identity;
  unsigned kind, marked;
  size_t accounted_bytes;
  size_t gc_bytes;
  union {
    /* String/Bytes/rich Error own bytes. Thin Error and structured ErrorRecord
       own source instead. Inspect kind before reading either interpretation. */
    struct {
      size_t length;
      /* SIZE_MAX means invalid UTF-8; equality with length identifies ASCII. */
      size_t scalar_length;
      unsigned char *bytes;
    };
    /* Private List/Map construction. Capacity stays valid if the published
       result takes ownership of the buffer. Error records use error_source. */
    struct {
      size_t collection_capacity;
      unsigned collection_building;
    };
    rinzu_source_position_t error_source;
  };
  union {
    struct {
      uint64_t message;
      uintptr_t native_pointer;
    };
    /* Error records have at least four slots and rich Errors exceed inline
       byte storage. Their otherwise unused second word owns optional trace. */
    struct {
      uint64_t trace_padding;
      rinzu_error_trace_t *error_trace;
    };
    /* String/Bytes/composite kinds do not use message/native_pointer. Small payloads
       fit in existing metadata storage without a second allocation. */
    rinzu_managed_slot_t inline_slot;
    rinzu_managed_map_index_t *map_index;
    unsigned char inline_bytes[sizeof(rinzu_managed_slot_t)];
  };
  /* Source is live together with composite fields in ErrorRecord. Only a
     thin Error owns diagnostic nodes; its primary trace uses error_trace. */
  union {
    struct {
      size_t type, count;
      uint32_t variant;
      int has_variant;
      rinzu_managed_slot_t *slots;
    };
    struct {
      int64_t error_primary;
      rinzu_diagnostic_node_t *error_head, *error_tail;
    };
  };
  struct rinzu_managed_value *mark_next;
  struct rinzu_managed_value *next;
  struct rinzu_managed_value *index_next;
} rinzu_managed_value_t;
typedef struct rinzu_managed_data rinzu_managed_data_t;
typedef struct {
  uint64_t file;
  uint32_t byte, line, column;
  const char *name;
  size_t name_length;
} rinzu_source_entry_t;
/* Bounded, weak cache for compiler-owned immutable literal bytes. Cache entries
   are identities, never GC roots or pointers to collectible metadata. */
#define RINZU_MANAGED_LITERAL_CAPACITY 128
#define RINZU_MANAGED_LITERAL_WAYS 4
#define RINZU_MANAGED_LOOKUP_CACHE_CAPACITY 4096
/* Private allocator retention only; use zero for a layout-matched control. */
#ifndef RINZU_MANAGED_METADATA_CACHE_CAPACITY
#define RINZU_MANAGED_METADATA_CACHE_CAPACITY 4096
#endif
_Static_assert(RINZU_MANAGED_METADATA_CACHE_CAPACITY >= 0 &&
                   RINZU_MANAGED_METADATA_CACHE_CAPACITY <= 4096,
               "managed metadata retention is bounded");
typedef struct {
  uint64_t identity;
  rinzu_managed_value_t *value;
} rinzu_managed_lookup_entry_t;
typedef struct {
  const unsigned char *bytes;
  size_t length;
  uint64_t identity;
} rinzu_managed_literal_t;
/* Private bounded weak enum hints; no roots or collectible pointers. */
#ifndef RINZU_MANAGED_SMALL_ENUM_MODE
#define RINZU_MANAGED_SMALL_ENUM_MODE 2
#endif
#define RINZU_MANAGED_SMALL_ENUM_CAPACITY 64
typedef struct {
  size_t type;
  uint64_t identity;
  uint32_t variant;
  uint16_t tag;
} rinzu_managed_small_enum_t;
/* Each Main task publishes its own frame chain into the same Domain heap.
   Entries are owned by the task and must be detached before their storage is
   released. A native continuation stack is never scanned for roots. */
typedef struct rinzu_managed_root_set {
  rinzu_managed_data_t *data;
  rinzu_managed_roots_t *roots;
  uint64_t future_owner;
  struct rinzu_managed_root_set *next, *previous;
} rinzu_managed_root_set_t;
/* Private links embedded at the start of native record loans. Root identities
   stay stable while payload storage moves; only native_record.h creates links. */
typedef struct rinzu_managed_record_loan {
  const uint64_t *identities;
  size_t count;
  int busy;
  struct rinzu_managed_record_loan *next;
  size_t storage_bytes, storage_alignment;
  unsigned storage_accounted;
} rinzu_managed_record_loan_t;
struct rinzu_managed_data {
  void *future_context;
  void (*future_begin)(void *);
  uint64_t (*future_mark)(void *, uint64_t);
  void (*future_sweep)(void *);
  int (*future_due)(void *);
  rinzu_managed_value_t *values;
  rinzu_managed_root_set_t *root_sets;
  rinzu_managed_record_loan_t *native_record_loans;
  rinzu_managed_record_loan_t *native_record_pool;
  size_t native_record_pool_bytes, native_record_allocations;
  rinzu_managed_frame_cache_t frame_cache;
  size_t published_root_count;
  const rinzu_value_schema_t *schema;
  const rinzu_source_entry_t *source_entries;
  size_t source_entry_count;
  size_t objects, peak_objects;
  size_t live_views, view_allocations;
  uint64_t collections;
  pthread_t owner;
  int owner_initialized;
  rinzu_managed_value_t **index;
  size_t index_capacity, index_accounted_bytes;
  /* A weak, direct-mapped hint into stable metadata. Its allocation survives
     index rebuilds; removal/free invalidates entries before metadata dies. */
  rinzu_managed_lookup_entry_t *lookup_cache;
  size_t lookup_cache_accounted_bytes;
  int lookup_cache_enabled;
  int inline_bytes_enabled;
  size_t heap_bytes;
  rinzu_gc_policy_t gc;
  rinzu_managed_literal_t *literals;
  size_t literal_accounted_bytes;
  unsigned literal_victim;
  rinzu_error_reference_t *error_references;
  /* Keep byte and String literal keys in separate weak tables. Their program
     addresses may coincide, but their managed kinds must never alias. */
  rinzu_managed_literal_t *byte_literals;
  unsigned byte_literal_victim;
  rinzu_managed_value_t *metadata_cache;
  size_t metadata_cache_count;
  int metadata_cache_disposing;
  rinzu_managed_small_enum_t *small_enums;
  size_t small_enum_accounted_bytes;
  char **program_argv;
  uint64_t program_arguments;
  int program_argc;
};
static inline int rinzu_managed_record_busy(const rinzu_managed_data_t *data, uint64_t identity) {
  for (const rinzu_managed_record_loan_t *loan = data->native_record_loans; loan; loan = loan->next)
    if (loan->busy)
      for (size_t index = 0; index < loan->count; ++index)
        if (loan->identities[index] == identity)
          return 1;
  return 0;
}
#if RINZU_MANAGED_METADATA_CACHE_CAPACITY
/* Resource reporting is optional and initialized before Domain creation. Keep
   its mutex/counter bookkeeping out of the ordinary metadata allocator frame. */
static __attribute__((noinline, cold)) void rinzu_managed_metadata_cache_uncharge(size_t bytes) {
  rinzu_resource_release(RINZU_RESOURCE_RUNTIME_CONTROL, bytes);
}
static __attribute__((noinline, cold)) void rinzu_managed_metadata_cache_charge(size_t bytes) {
  rinzu_resource_acquire(RINZU_RESOURCE_RUNTIME_CONTROL, bytes);
}
#endif
static __attribute__((noinline,
                      cold)) void rinzu_managed_owner_initialize(rinzu_managed_data_t *data) {
  data->owner = pthread_self();
  data->owner_initialized = 1;
  const char *cache = getenv("RINZU_MANAGED_LOOKUP_CACHE");
  data->lookup_cache_enabled = !cache || strcmp(cache, "0") != 0;
  const char *inline_bytes = getenv("RINZU_MANAGED_INLINE_BYTES");
  data->inline_bytes_enabled = !inline_bytes || strcmp(inline_bytes, "0") != 0;
  const char *stress = getenv("RINZU_GC_STRESS");
  data->gc = rinzu_gc_policy_init(stress && strcmp(stress, "1") == 0);
}
static inline void rinzu_managed_owner(rinzu_managed_data_t *data) {
  if (!data->owner_initialized)
    rinzu_managed_owner_initialize(data);
  if (!rinzu_thread_equal(data->owner, pthread_self()))
    rinzu_failure_fatal();
}
static inline void rinzu_managed_native_owner(rinzu_managed_data_t *data) {
  if (!data)
    rinzu_interop_fatal();
  if (!data->owner_initialized)
    rinzu_managed_owner_initialize(data);
  if (!rinzu_thread_equal(data->owner, pthread_self()))
    rinzu_interop_fatal();
}
static inline void rinzu_managed_attach_roots(rinzu_managed_data_t *data,
                                              rinzu_managed_root_set_t *entry,
                                              rinzu_managed_roots_t *roots) {
  rinzu_managed_owner(data);
  if (!entry || !roots || entry->data || entry->roots || entry->next || entry->previous ||
      roots->domain_entry || roots->domain_published_count || roots->frame_cache)
    rinzu_failure_fatal();
  size_t count = rinzu_managed_frame_chain_count(roots);
  if (count > SIZE_MAX - data->published_root_count)
    rinzu_resource_fatal();
  entry->data = data;
  entry->roots = roots;
  entry->next = data->root_sets;
  if (entry->next)
    entry->next->previous = entry;
  data->root_sets = entry;
  roots->domain_entry = entry;
  data->published_root_count += count;
  roots->domain_published_count = &data->published_root_count;
#if RINZU_MANAGED_FRAME_CACHE_COUNT
  roots->frame_cache = &data->frame_cache;
#endif
}
static inline void rinzu_managed_detach_roots(rinzu_managed_data_t *data,
                                              rinzu_managed_root_set_t *entry) {
  rinzu_managed_owner(data);
  if (!entry || entry->data != data || !entry->roots || entry->roots->current ||
      entry->roots->depth || entry->roots->domain_entry != entry ||
      entry->roots->domain_published_count != &data->published_root_count ||
      (entry->previous ? entry->previous->next != entry : data->root_sets != entry) ||
      (entry->next && entry->next->previous != entry))
    rinzu_failure_fatal();
  if (entry->previous)
    entry->previous->next = entry->next;
  else
    data->root_sets = entry->next;
  if (entry->next)
    entry->next->previous = entry->previous;
  entry->roots->domain_entry = NULL;
  entry->roots->domain_published_count = NULL;
  entry->roots->frame_cache = NULL;
  *entry = (rinzu_managed_root_set_t){0};
}
static inline size_t rinzu_managed_native_root_count(rinzu_managed_data_t *data,
                                                     const rinzu_managed_roots_t *roots) {
  rinzu_managed_owner(data);
  size_t count = data->published_root_count;
  if (roots->domain_entry) {
    if (roots->domain_entry->data != data ||
        roots->domain_published_count != &data->published_root_count)
      rinzu_failure_fatal();
  } else {
    if (roots->domain_published_count)
      rinzu_failure_fatal();
    size_t current = rinzu_managed_frame_chain_count(roots);
    if (current > SIZE_MAX - count)
      rinzu_resource_fatal();
    count += current;
  }
  return count;
}
/* Index only stable metadata. Payloads and composite slots still move at every
   collection; typed references and Domain ownership are checked as before. */
static inline size_t rinzu_managed_index_bucket(uint64_t identity, size_t capacity) {
  identity = (identity ^ (identity >> 30)) * UINT64_C(0xbf58476d1ce4e5b9);
  identity = (identity ^ (identity >> 27)) * UINT64_C(0x94d049bb133111eb);
  identity ^= identity >> 31;
  return (size_t)(identity & (capacity - 1));
}
static inline void rinzu_managed_index_release(rinzu_managed_data_t *data) {
  if (data->index_accounted_bytes)
    rinzu_resource_release(RINZU_RESOURCE_MANAGED, data->index_accounted_bytes);
  free(data->index);
  if (data->lookup_cache)
    free(data->lookup_cache);
  data->lookup_cache = NULL;
  data->lookup_cache_accounted_bytes = 0;
  data->index = NULL;
  data->index_capacity = data->index_accounted_bytes = 0;
}
static inline void rinzu_managed_index_resize(rinzu_managed_data_t *data, size_t capacity) {
  if (capacity < 32 || (capacity & (capacity - 1)) || capacity > SIZE_MAX / sizeof(*data->index))
    rinzu_failure_fatal();
  if (data->index_accounted_bytes < data->lookup_cache_accounted_bytes)
    rinzu_failure_fatal();
  size_t previous_index_bytes = data->index_accounted_bytes - data->lookup_cache_accounted_bytes;
  size_t bytes = capacity * sizeof(*data->index);
  rinzu_managed_value_t **index = rinzu_failure_allocate(bytes);
  rinzu_gc_allocated(&data->gc, bytes);
  size_t accounted = 0;
  if (atomic_load_explicit(&rinzu_resource_enabled, memory_order_relaxed)) {
    rinzu_resource_acquire(RINZU_RESOURCE_MANAGED, bytes);
    accounted = bytes;
  }
  if (data->lookup_cache_enabled && !data->lookup_cache) {
    size_t cache_bytes = RINZU_MANAGED_LOOKUP_CACHE_CAPACITY * sizeof(*data->lookup_cache);
    data->lookup_cache = rinzu_failure_allocate(cache_bytes);
    rinzu_gc_allocated(&data->gc, cache_bytes);
    if (atomic_load_explicit(&rinzu_resource_enabled, memory_order_relaxed)) {
      rinzu_resource_acquire(RINZU_RESOURCE_MANAGED, cache_bytes);
      data->lookup_cache_accounted_bytes = cache_bytes;
    }
  }
  for (rinzu_managed_value_t *value = data->values; value; value = value->next) {
    size_t bucket = rinzu_managed_index_bucket(value->identity, capacity);
    value->index_next = index[bucket];
    index[bucket] = value;
  }
  if (previous_index_bytes)
    rinzu_resource_release(RINZU_RESOURCE_MANAGED, previous_index_bytes);
  free(data->index);
  data->index = index;
  data->index_capacity = capacity;
  data->index_accounted_bytes = accounted + data->lookup_cache_accounted_bytes;
}
static inline void rinzu_managed_index_remove(rinzu_managed_data_t *data,
                                              rinzu_managed_value_t *value) {
  if (!data->index || !data->index_capacity)
    rinzu_failure_fatal();
  size_t bucket = rinzu_managed_index_bucket(value->identity, data->index_capacity);
  rinzu_managed_value_t **cursor = &data->index[bucket];
  while (*cursor && *cursor != value)
    cursor = &(*cursor)->index_next;
  if (!*cursor)
    rinzu_failure_fatal();
  *cursor = value->index_next;
  value->index_next = NULL;
  if (data->lookup_cache) {
    size_t slot = (size_t)value->identity & (RINZU_MANAGED_LOOKUP_CACHE_CAPACITY - 1);
    if (data->lookup_cache[slot].value == value)
      data->lookup_cache[slot] = (rinzu_managed_lookup_entry_t){0};
  }
}
static inline void rinzu_managed_index_trim(rinzu_managed_data_t *data) {
  if (!data->objects) {
    if (data->values)
      rinzu_failure_fatal();
    for (size_t bucket = 0; bucket < data->index_capacity; ++bucket)
      if (data->index[bucket])
        rinzu_failure_fatal();
    rinzu_managed_index_release(data);
    return;
  }
  /* Keep existing small tables across sparse collections to avoid rebuilding
     them on the next allocation burst. Larger tables keep the usual load
     threshold; the empty-heap path above frees both arrays. This loop never
     allocates a minimum table (16384 buckets retain at most 128 KiB). */
  size_t capacity = data->index_capacity;
  while (capacity > 16384 && data->objects < capacity / 8)
    capacity /= 2;
  if (capacity != data->index_capacity)
    rinzu_managed_index_resize(data, capacity);
}
/* The caller has already checked this Domain's Owner and cannot reenter or
   yield while using this lookup. External operations use managed_lookup. */
#if defined(__clang__)
#if __has_attribute(preserve_most) && (defined(__aarch64__) || defined(__x86_64__))
#define RINZU_MANAGED_LOOKUP_SLOW_ABI __attribute__((preserve_most))
#endif
#endif
#ifndef RINZU_MANAGED_LOOKUP_SLOW_ABI
#define RINZU_MANAGED_LOOKUP_SLOW_ABI
#endif
static __attribute__((noinline, cold)) RINZU_MANAGED_LOOKUP_SLOW_ABI rinzu_managed_value_t *
rinzu_managed_lookup_uncached_owned(rinzu_managed_data_t *data, uint64_t identity) {
  if (!data->index_capacity)
    return NULL;
  size_t bucket = rinzu_managed_index_bucket(identity, data->index_capacity);
  for (rinzu_managed_value_t *value = data->index[bucket]; value; value = value->index_next)
    if (value->identity == identity) {
      if (data->lookup_cache) {
        size_t slot = (size_t)identity & (RINZU_MANAGED_LOOKUP_CACHE_CAPACITY - 1);
        data->lookup_cache[slot] = (rinzu_managed_lookup_entry_t){identity, value};
      }
      return value;
    }
  return NULL;
}
#undef RINZU_MANAGED_LOOKUP_SLOW_ABI
static inline __attribute__((always_inline))
rinzu_managed_value_t *rinzu_managed_lookup_owned(rinzu_managed_data_t *data, uint64_t identity) {
  if (data->lookup_cache) {
    rinzu_managed_lookup_entry_t cached =
        data->lookup_cache[(size_t)identity & (RINZU_MANAGED_LOOKUP_CACHE_CAPACITY - 1)];
    if (cached.identity == identity)
      return cached.value;
  }
  return rinzu_managed_lookup_uncached_owned(data, identity);
}
static inline rinzu_managed_value_t *rinzu_managed_lookup(rinzu_managed_data_t *data,
                                                          uint64_t identity) {
  rinzu_managed_owner(data);
  return rinzu_managed_lookup_owned(data, identity);
}
static inline rinzu_managed_value_t *rinzu_managed_find(rinzu_managed_data_t *data,
                                                        uint64_t identity) {
  rinzu_managed_value_t *value = rinzu_managed_lookup(data, identity);
  if (value)
    return value;
  rinzu_failure_fatal();
  return NULL;
}
/* Internal operations have checked the Owner and cannot yield, reenter native
   code, or collect until their result is published. */
static inline rinzu_managed_value_t *rinzu_managed_new_owned(rinzu_managed_data_t *data,
                                                             unsigned kind) {
  uint64_t previous = atomic_load_explicit(&rinzu_managed_next_identity, memory_order_relaxed);
  do {
    if (previous >= INT64_MAX)
      rinzu_failure_fatal();
  } while (!atomic_compare_exchange_weak_explicit(&rinzu_managed_next_identity, &previous,
                                                  previous + 1, memory_order_relaxed,
                                                  memory_order_relaxed));
  if (data->objects == SIZE_MAX)
    rinzu_failure_fatal();
  if (!data->index_capacity || data->objects + 1 > data->index_capacity / 2) {
    size_t capacity = data->index_capacity ? data->index_capacity : 32;
    while (data->objects + 1 > capacity / 2) {
      if (capacity > SIZE_MAX / 2)
        rinzu_failure_fatal();
      capacity *= 2;
    }
    rinzu_managed_index_resize(data, capacity);
  }
  rinzu_managed_value_t *value;
#if RINZU_MANAGED_METADATA_CACHE_CAPACITY
  if (data->metadata_cache) {
    if (!data->metadata_cache_count)
      rinzu_failure_fatal();
    value = data->metadata_cache;
    data->metadata_cache = value->next;
    --data->metadata_cache_count;
    if (value->accounted_bytes)
      rinzu_managed_metadata_cache_uncharge(value->accounted_bytes);
    /* Same zero-initialization contract as the existing allocator. No retired
       edge, Error trace, mark bit, map index or old identity may escape. */
    memset(value, 0, sizeof(*value));
  } else
#endif
    value = rinzu_failure_allocate(sizeof(*value));
  if (data->heap_bytes > SIZE_MAX - sizeof(*value))
    rinzu_resource_fatal();
  value->gc_bytes = sizeof(*value);
  data->heap_bytes += sizeof(*value);
  rinzu_gc_allocated(&data->gc, sizeof(*value));
  value->identity = previous + 1;
  value->kind = kind;
  if (atomic_load_explicit(&rinzu_resource_enabled, memory_order_relaxed)) {
    value->accounted_bytes = sizeof(*value);
    rinzu_resource_acquire(RINZU_RESOURCE_MANAGED, sizeof(*value));
    rinzu_resource_acquire(RINZU_RESOURCE_MANAGED_OBJECTS, 1);
  }
  value->next = data->values;
  data->values = value;
  size_t bucket = rinzu_managed_index_bucket(value->identity, data->index_capacity);
  value->index_next = data->index[bucket];
  data->index[bucket] = value;
  data->objects += 1;
  if (data->objects > data->peak_objects)
    data->peak_objects = data->objects;
  /* This fresh full identity is already in the Domain index. The weak hint
     certifies membership, not typed payload validity or GC reachability. */
  if (data->lookup_cache) {
    size_t slot = (size_t)value->identity & (RINZU_MANAGED_LOOKUP_CACHE_CAPACITY - 1);
    data->lookup_cache[slot] = (rinzu_managed_lookup_entry_t){value->identity, value};
  }
  return value;
}
static inline rinzu_managed_value_t *rinzu_managed_new(rinzu_managed_data_t *data, unsigned kind) {
  rinzu_managed_owner(data);
  return rinzu_managed_new_owned(data, kind);
}
static inline void rinzu_managed_account_payload(rinzu_managed_data_t *data,
                                                 rinzu_managed_value_t *value, size_t bytes) {
  if (value->gc_bytes > SIZE_MAX - bytes || data->heap_bytes > SIZE_MAX - bytes)
    rinzu_resource_fatal();
  value->gc_bytes += bytes;
  data->heap_bytes += bytes;
  rinzu_gc_allocated(&data->gc, bytes);
  if (!value->accounted_bytes)
    return;
  if (value->accounted_bytes > SIZE_MAX - bytes)
    rinzu_resource_fatal();
  value->accounted_bytes += bytes;
  rinzu_resource_acquire(RINZU_RESOURCE_MANAGED, bytes);
}
static inline void rinzu_managed_unaccount_payload(rinzu_managed_data_t *data,
                                                   rinzu_managed_value_t *value, size_t bytes) {
  if (bytes > value->gc_bytes || bytes > data->heap_bytes)
    rinzu_failure_fatal();
  value->gc_bytes -= bytes;
  data->heap_bytes -= bytes;
  if (value->accounted_bytes) {
    if (bytes > value->accounted_bytes)
      rinzu_failure_fatal();
    value->accounted_bytes -= bytes;
    rinzu_resource_release(RINZU_RESOURCE_MANAGED, bytes);
  }
}
/* Allocate the final String once, then fill it inside this non-yielding owned
   operation. The caller supplies validated immutable UTF-8 scalar metadata. */
static inline rinzu_managed_value_t *
rinzu_managed_string_allocate_owned(rinzu_managed_data_t *data, size_t length, size_t scalars) {
  if (length == SIZE_MAX || (scalars != SIZE_MAX && scalars > length))
    rinzu_failure_fatal();
  rinzu_managed_value_t *value = rinzu_managed_new_owned(data, RINZU_MANAGED_STRING);
  value->length = length;
  value->scalar_length = scalars;
  if (length < sizeof(value->inline_bytes))
    value->bytes = value->inline_bytes;
  else {
    value->bytes = rinzu_failure_allocate(length + 1);
    rinzu_managed_account_payload(data, value, length + 1);
  }
  return value;
}
static inline uint64_t rinzu_managed_string_copy_owned(rinzu_managed_data_t *data,
                                                       const unsigned char *bytes, size_t length,
                                                       size_t scalars) {
  if (length && !bytes)
    rinzu_failure_fatal();
  rinzu_managed_value_t *value = rinzu_managed_string_allocate_owned(data, length, scalars);
  if (length)
    memcpy(value->bytes, bytes, length);
  return value->identity;
}
/* Receive an already copied, NUL-padded native snapshot. Never adopt borrowed
   native memory; the allocation and its accounting become managed here. */
static inline uint64_t rinzu_managed_string_take_owned(rinzu_managed_data_t *data,
                                                       unsigned char *bytes, size_t length,
                                                       size_t scalars) {
  if (!bytes || length == SIZE_MAX || scalars > length || bytes[length])
    rinzu_failure_fatal();
  rinzu_managed_value_t *value = rinzu_managed_new_owned(data, RINZU_MANAGED_STRING);
  value->length = length;
  value->scalar_length = scalars;
  value->bytes = bytes;
  rinzu_managed_account_payload(data, value, length + 1);
  return value->identity;
}
static inline uint64_t rinzu_managed_string(rinzu_managed_data_t *data, const unsigned char *bytes,
                                            size_t length) {
  rinzu_managed_owner(data);
  return rinzu_managed_string_copy_owned(data, bytes, length,
                                         rinzu_utf8_scalar_length(bytes, length));
}
/* Owner-checked, non-collecting lookup for one kind's compiler-owned literals.
   Never retain metadata pointers: removal/rollback may retire the weak ID.
   Inline this shared key logic so the established String hit keeps its direct
   table loads; each caller supplies a distinct table and replacement cursor.
   Return the hit control fact, rather than making the caller
   re-test a reloaded identity for zero after its liveness lookup. */
static inline __attribute__((always_inline)) int
rinzu_managed_literal_lookup_owned(rinzu_managed_data_t *data, rinzu_managed_literal_t **table,
                                   unsigned *victim, const unsigned char *bytes, size_t length,
                                   rinzu_managed_literal_t **slot, uint64_t *hit) {
  if (!*table) {
    size_t size = RINZU_MANAGED_LITERAL_CAPACITY * sizeof(**table);
    *table = rinzu_failure_allocate(size);
    if (data->heap_bytes > SIZE_MAX - size)
      rinzu_resource_fatal();
    data->heap_bytes += size;
    rinzu_gc_allocated(&data->gc, size);
    if (atomic_load_explicit(&rinzu_resource_enabled, memory_order_relaxed)) {
      data->literal_accounted_bytes += size;
      rinzu_resource_acquire(RINZU_RESOURCE_MANAGED, size);
    }
  }
  size_t bucket =
      RINZU_MANAGED_LITERAL_WAYS *
      rinzu_managed_index_bucket((uint64_t)(uintptr_t)bytes ^ (uint64_t)length,
                                 RINZU_MANAGED_LITERAL_CAPACITY / RINZU_MANAGED_LITERAL_WAYS);
  rinzu_managed_literal_t *literal = NULL;
  for (size_t way = 0; way < RINZU_MANAGED_LITERAL_WAYS; ++way) {
    rinzu_managed_literal_t *entry = &(*table)[bucket + way];
    if (entry->identity && entry->bytes == bytes && entry->length == length) {
      if (rinzu_managed_lookup_owned(data, entry->identity)) {
        *hit = entry->identity;
        return 1;
      }
      literal = entry;
      break;
    }
    if (!entry->identity && !literal)
      literal = entry;
  }
  if (!literal) {
    literal = &(*table)[bucket + *victim];
    *victim = (*victim + 1) % RINZU_MANAGED_LITERAL_WAYS;
  }
  *slot = literal;
  return 0;
}
/* Only generated literal helpers may call this: bytes have immutable program
   lifetime. Dynamic/native Strings continue to receive their own snapshots. */
static inline uint64_t rinzu_managed_string_literal(rinzu_managed_data_t *data,
                                                    const unsigned char *bytes, size_t length) {
  rinzu_managed_owner(data);
  if (length == SIZE_MAX || (length && !bytes))
    rinzu_failure_fatal();
  uint64_t hit;
  rinzu_managed_literal_t *literal;
  if (rinzu_managed_literal_lookup_owned(data, &data->literals, &data->literal_victim, bytes,
                                         length, &literal, &hit))
    return hit;
  uint64_t identity =
      rinzu_managed_string_copy_owned(data, bytes, length, rinzu_utf8_scalar_length(bytes, length));
  *literal = (rinzu_managed_literal_t){bytes, length, identity};
  return identity;
}
static inline rinzu_managed_value_t *rinzu_managed_string_value(rinzu_managed_data_t *data,
                                                                uint64_t identity) {
  rinzu_managed_value_t *value = rinzu_managed_find(data, identity);
  if (value->kind != RINZU_MANAGED_STRING || value->scalar_length == SIZE_MAX)
    rinzu_failure_fatal();
  return value;
}
static inline int rinzu_managed_string_equal(rinzu_managed_data_t *data, uint64_t left,
                                             uint64_t right) {
  rinzu_managed_value_t *a = rinzu_managed_string_value(data, left),
                        *b = rinzu_managed_string_value(data, right);
  return a->length == b->length && memcmp(a->bytes, b->bytes, a->length) == 0;
}
static inline uint64_t rinzu_managed_string_hash(rinzu_managed_data_t *data, uint64_t identity) {
  rinzu_managed_value_t *value = rinzu_managed_string_value(data, identity);
  rinzu_key_hasher_t hash = rinzu_key_init();
  if (rinzu_key_string(&hash, value->bytes, value->length))
    rinzu_failure_fatal();
  return hash.hash;
}
static inline const char *rinzu_managed_error_text(int64_t cause) {
  switch (cause) {
  case 5:
    return "numeric conversion is out of range or loses required integer precision";
  case 8:
    return "native value is NULL";
  case 9:
    return "native string is not valid UTF-8";
  case 10:
    return "native record is already in use";
  case 11:
    return "child task failure was not observed";
  case 12:
    return "I/O operation failed";
  case 13:
    return "another supplant call is still active";
  case 15:
    return "user-defined Error";
  case 14:
    return "local binding has no value";
  default:
    rinzu_failure_fatal();
    return "";
  }
}
static inline uint64_t rinzu_managed_error(rinzu_managed_data_t *data,
                                           const rinzu_failure_t *failure) {
  if (failure->primary_error) {
    if (failure->head || failure->primary_error->owner != data)
      rinzu_failure_fatal();
    (void)rinzu_managed_find(data, failure->primary_error->handle);
    return failure->primary_error->handle;
  }
  rinzu_failure_state_t check = {0};
  check.current = *failure;
  if (!rinzu_failure_recoverable(&check))
    rinzu_failure_fatal();
  static const char separator[] = "; cleanup: ";
  const char *primary = rinzu_managed_error_text(failure->primary);
  size_t length = strlen(primary);
  for (rinzu_diagnostic_node_t *node = failure->head; node; node = node->next) {
    size_t extra = sizeof(separator) - 1 + strlen(rinzu_managed_error_text(node->cause));
    if (extra > SIZE_MAX - length)
      rinzu_failure_fatal();
    length += extra;
  }
  if (length == SIZE_MAX)
    rinzu_failure_fatal();
  unsigned char *bytes = rinzu_failure_allocate(length + 1);
  size_t offset = strlen(primary);
  memcpy(bytes, primary, offset);
  for (rinzu_diagnostic_node_t *node = failure->head; node; node = node->next) {
    memcpy(bytes + offset, separator, sizeof(separator) - 1);
    offset += sizeof(separator) - 1;
    const char *text = rinzu_managed_error_text(node->cause);
    size_t count = strlen(text);
    memcpy(bytes + offset, text, count);
    offset += count;
  }
  uint64_t message = rinzu_managed_string(data, bytes, length);
  free(bytes);
  rinzu_managed_value_t *value = rinzu_managed_new(data, RINZU_MANAGED_ERROR);
  value->message = message;
  value->error_primary = failure->primary;
  value->error_source = failure->primary_source;
  value->error_trace = rinzu_error_trace_copy(failure->primary_trace);
  for (rinzu_diagnostic_node_t *node = failure->head; node; node = node->next) {
    rinzu_diagnostic_node_t *copy = rinzu_failure_allocate(sizeof(*copy));
    rinzu_managed_account_payload(data, value, sizeof(*copy));
    copy->cause = node->cause;
    copy->source = node->source;
    copy->trace = rinzu_error_trace_copy(node->trace);
    if (value->error_tail)
      value->error_tail->next = copy;
    else
      value->error_head = copy;
    value->error_tail = copy;
  }
  return value->identity;
}
static inline uint64_t rinzu_managed_rich_error(rinzu_managed_data_t *data,
                                                const unsigned char *bytes, size_t length) {
  if (length >= SIZE_MAX || !rinzu_error_value_valid(bytes, length))
    rinzu_failure_fatal();
  rinzu_managed_value_t *value = rinzu_managed_new(data, RINZU_MANAGED_RICH_ERROR);
  value->bytes = rinzu_failure_allocate(length + 1);
  memcpy(value->bytes, bytes, length);
  value->length = length;
  rinzu_managed_account_payload(data, value, length + 1);
  return value->identity;
}
static inline rinzu_error_buffer_t rinzu_managed_error_blob(rinzu_managed_data_t *data,
                                                            rinzu_managed_value_t *value) {
  if (value->kind == RINZU_MANAGED_RICH_ERROR) {
    rinzu_error_buffer_t result = {0};
    rinzu_error_append(&result, value->bytes, value->length);
    return result;
  }
  if (value->kind != RINZU_MANAGED_ERROR)
    rinzu_failure_fatal();
  uint32_t count = 0;
  for (rinzu_diagnostic_node_t *p = value->error_head; p; p = p->next) {
    if (count == UINT32_MAX - 1)
      rinzu_failure_fatal();
    count++;
  }
  rinzu_error_buffer_t result = {0}, indices = {0};
  rinzu_error_write(&result, count + 1, 4);
  for (rinzu_diagnostic_node_t *p = value->error_head; p; p = p->next) {
    const char *message = rinzu_managed_error_text(p->cause);
    rinzu_error_node_t node = {.code = rinzu_error_code_from_failure(p->cause),
                               .message = (const unsigned char *)message,
                               .message_length = strlen(message),
                               .source = p->source,
                               .cause = UINT32_MAX};
    rinzu_error_encode_node(&result, &node, 0);
    rinzu_error_write(&indices, indices.length / 4, 4);
  }
  rinzu_managed_value_t *message = rinzu_managed_string_value(data, value->message);
  rinzu_error_node_t node = {.code = rinzu_error_code_from_failure(value->error_primary),
                             .message = message->bytes,
                             .message_length = message->length,
                             .source = value->error_source,
                             .cause = UINT32_MAX,
                             .cleanup_count = count,
                             .cleanup = indices.bytes};
  rinzu_error_encode_node(&result, &node, 0);
  free(indices.bytes);
  return result;
}
static inline uint64_t rinzu_managed_message(rinzu_managed_data_t *data, uint64_t error) {
  rinzu_managed_value_t *value = rinzu_managed_find(data, error);
  if (value->kind == RINZU_MANAGED_COMPOSITE && data->schema && value->type < data->schema->count &&
      data->schema->types[value->type].kind == RINZU_VALUE_ERROR_RECORD) {
    if (value->count < 4 || !value->slots[1].reference)
      rinzu_failure_fatal();
    return value->slots[1].bits;
  }
  if (value->kind == RINZU_MANAGED_RICH_ERROR) {
    rinzu_error_node_t node = rinzu_error_node(value->bytes, value->length,
                                               (uint32_t)rinzu_error_integer(value->bytes, 4) - 1);
    return rinzu_managed_string(data, node.message, node.message_length);
  }
  if (value->kind != RINZU_MANAGED_ERROR)
    rinzu_failure_fatal();
  return value->message;
}
static inline void rinzu_managed_mark(rinzu_managed_data_t *data, uint64_t identity) {
  if (identity > RINZU_MANAGED_FUTURE_BASE && identity < RINZU_MANAGED_HANDLE_BASE &&
      data->future_mark)
    identity = data->future_mark(data->future_context, identity);
  if (identity <= RINZU_MANAGED_HANDLE_BASE || identity > INT64_MAX)
    return;
  rinzu_managed_value_t *pending = rinzu_managed_find(data, identity);
  if (pending->marked)
    return;
  pending->marked = 1;
  pending->mark_next = NULL;
  while (pending) {
    rinzu_managed_value_t *value = pending;
    pending = value->mark_next;
    value->mark_next = NULL;
    size_t count = value->kind == RINZU_MANAGED_ERROR
                       ? 1
                       : (value->kind == RINZU_MANAGED_COMPOSITE ? value->count : 0);
    for (size_t index = 0; index < count; ++index) {
      if (value->kind == RINZU_MANAGED_COMPOSITE && !value->slots[index].reference) {
        if (!data->future_mark)
          continue;
        const rinzu_value_type_t *type = &data->schema->types[value->type];
        size_t child_type = type->kind == RINZU_VALUE_LIST  ? type->element
                            : type->kind == RINZU_VALUE_MAP ? (index % 2 ? type->value : type->key)
                            : type->kind == RINZU_VALUE_ENUM
                                ? type->variants[value->variant].types[index]
                                : type->fields[index];
        const rinzu_value_type_t *child = &data->schema->types[child_type];
        if (child->kind != RINZU_VALUE_OTHER || child->scalar_kind != RINZU_VALUE_OTHER_FUTURE)
          continue;
        uint64_t result = data->future_mark(data->future_context, value->slots[index].bits);
        if (!result)
          continue;
        rinzu_managed_value_t *next = rinzu_managed_find(data, result);
        if (!next->marked) {
          next->marked = 1;
          next->mark_next = pending;
          pending = next;
        }
        continue;
      }
      uint64_t child =
          value->kind == RINZU_MANAGED_ERROR ? value->message : value->slots[index].bits;
      rinzu_managed_value_t *next = rinzu_managed_find(data, child);
      if (!next->marked) {
        next->marked = 1;
        next->mark_next = pending;
        pending = next;
      }
    }
  }
}
static inline void rinzu_managed_value_release(rinzu_managed_data_t *data,
                                               rinzu_managed_value_t *value) {
  /* Metadata stays at a stable address when payloads move. This is a weak
     lookup hint, never a tracing root. Remove before every physical release,
     including rollback and final Domain disposal, not just a GC sweep. */
  if (data->lookup_cache) {
    size_t slot = (size_t)value->identity & (RINZU_MANAGED_LOOKUP_CACHE_CAPACITY - 1);
    if (data->lookup_cache[slot].value == value)
      data->lookup_cache[slot] = (rinzu_managed_lookup_entry_t){0};
  }
  if (data->heap_bytes < value->gc_bytes)
    rinzu_failure_fatal();
  data->heap_bytes -= value->gc_bytes;
  if (value->accounted_bytes) {
    rinzu_resource_release(RINZU_RESOURCE_MANAGED, value->accounted_bytes);
    rinzu_resource_release(RINZU_RESOURCE_MANAGED_OBJECTS, 1);
  }
  if ((value->kind == RINZU_MANAGED_STRING || value->kind == RINZU_MANAGED_BYTES ||
       value->kind == RINZU_MANAGED_RICH_ERROR) &&
      value->bytes != value->inline_bytes)
    free(value->bytes);
  if (value->kind == RINZU_MANAGED_COMPOSITE && value->slots != &value->inline_slot)
    free(value->slots);
  if (value->kind == RINZU_MANAGED_COMPOSITE && data->schema && value->type < data->schema->count &&
      data->schema->types[value->type].kind == RINZU_VALUE_MAP)
    free(value->map_index);
  if (value->kind == RINZU_MANAGED_ERROR || value->kind == RINZU_MANAGED_RICH_ERROR ||
      (value->kind == RINZU_MANAGED_COMPOSITE && value->count >= 4 && data->schema &&
       value->type < data->schema->count &&
       data->schema->types[value->type].kind == RINZU_VALUE_ERROR_RECORD))
    rinzu_error_trace_release(value->error_trace);
  /* Metadata never owns a primary_error/primary_trace Failure reference:
     managed_error returns existing primary_error unchanged and retains the
     primary trace separately. Cleanup nodes retain their own references. */
  if (value->kind == RINZU_MANAGED_ERROR && value->error_head)
    rinzu_diagnostic_nodes_release(value->error_head);
#if RINZU_MANAGED_METADATA_CACHE_CAPACITY
  if (!data->metadata_cache_disposing &&
      data->metadata_cache_count < RINZU_MANAGED_METADATA_CACHE_CAPACITY) {
    /* The object and all payloads are already released. Transfer retained
       metadata bytes to native runtime-control accounting, never a GC root or
       a live language object. Original mutator GC pacing remains unchanged. */
    size_t charged = value->accounted_bytes ? sizeof(*value) : 0;
    if (charged)
      rinzu_managed_metadata_cache_charge(charged);
    value->accounted_bytes = charged;
    value->next = data->metadata_cache;
    data->metadata_cache = value;
    ++data->metadata_cache_count;
    return;
  }
#endif
  free(value);
}
/* GC destinations are fully overwritten before publication. Both callers
   have positive sizes: length+1 for valid byte payloads and nonempty slots.
   Allocate before retiring the source so relocation never overlaps it. */
static inline __attribute__((always_inline)) void *rinzu_managed_relocate_copy(const void *source,
                                                                               size_t size) {
  void *destination = malloc(size);
  if (!destination)
    rinzu_resource_fatal();
  memcpy(destination, source, size);
  return destination;
}
/* Normal collection retains survivor payload storage; movement stress still
   relocates it. Both modes validate borrowing and trace only published typed
   roots/transitive fields. Native stacks are never scanned. */
static inline void rinzu_managed_collect(rinzu_managed_data_t *data,
                                         const rinzu_managed_roots_t *roots) {
  rinzu_managed_owner(data);
  if (data->live_views || data->view_allocations)
    rinzu_failure_fatal();
  int relocate = data->gc.stress;
  size_t root_bytes = 0;
  if (data->program_arguments) {
    rinzu_managed_mark(data, data->program_arguments);
    root_bytes = sizeof(data->program_arguments);
  }
  if (data->future_begin)
    data->future_begin(data->future_context);
  rinzu_error_reference_t **errors = &data->error_references;
  while (*errors) {
    rinzu_error_reference_t *error = *errors;
    if (!error->references) {
      *errors = error->next;
      free(error);
    } else {
      if (error->owner != data)
        rinzu_failure_fatal();
      rinzu_managed_mark(data, error->handle);
      root_bytes = rinzu_gc_add(root_bytes, sizeof(error->handle));
      errors = &error->next;
    }
  }
  for (rinzu_managed_root_set_t *entry = data->root_sets; entry; entry = entry->next) {
    if (entry->data != data || !entry->roots)
      rinzu_failure_fatal();
    if (entry->future_owner && data->future_mark)
      continue;
    for (rinzu_managed_frame_t *frame = entry->roots->current; frame; frame = frame->previous) {
      if (frame->writing)
        rinzu_failure_fatal();
      root_bytes = rinzu_gc_add(root_bytes, frame->count * sizeof(*frame->roots));
      for (size_t index = 0; index < frame->count; ++index)
        rinzu_managed_mark(data, frame->roots[index]);
    }
  }
  for (rinzu_managed_frame_t *frame = roots->current; frame; frame = frame->previous) {
    if (frame->writing)
      rinzu_failure_fatal();
    root_bytes = rinzu_gc_add(root_bytes, frame->count * sizeof(*frame->roots));
    for (size_t index = 0; index < frame->count; ++index)
      rinzu_managed_mark(data, frame->roots[index]);
  }
  if (data->future_sweep)
    data->future_sweep(data->future_context);
  rinzu_managed_value_t **cursor = &data->values;
  while (*cursor) {
    rinzu_managed_value_t *value = *cursor;
    if (!value->marked) {
      *cursor = value->next;
      rinzu_managed_index_remove(data, value);
      rinzu_managed_value_release(data, value);
      data->objects -= 1;
      continue;
    }
    if (relocate && (value->kind == RINZU_MANAGED_STRING || value->kind == RINZU_MANAGED_BYTES ||
                     value->kind == RINZU_MANAGED_RICH_ERROR)) {
      unsigned char *moved = rinzu_managed_relocate_copy(value->bytes, value->length + 1);
      if (value->bytes == value->inline_bytes)
        rinzu_managed_account_payload(data, value, value->length + 1);
      else {
        size_t capacity = value->gc_bytes - sizeof(*value);
        if (capacity > value->length + 1)
          rinzu_managed_unaccount_payload(data, value, capacity - value->length - 1);
        free(value->bytes);
      }
      value->bytes = moved;
    }
    if (relocate && value->kind == RINZU_MANAGED_COMPOSITE && value->count) {
      unsigned kind = data->schema ? data->schema->types[value->type].kind : 0;
      size_t capacity = value->count;
      if ((kind == RINZU_VALUE_LIST || kind == RINZU_VALUE_MAP) && value->collection_building)
        capacity = value->collection_capacity;
      if (capacity > SIZE_MAX / sizeof(*value->slots))
        rinzu_failure_fatal();
      rinzu_managed_slot_t *moved = rinzu_failure_allocate(capacity * sizeof(*moved));
      memcpy(moved, value->slots, value->count * sizeof(*moved));
      if (value->slots == &value->inline_slot)
        rinzu_managed_account_payload(data, value, capacity * sizeof(*moved));
      size_t expected = sizeof(*value) + capacity * sizeof(*moved);
      if (kind == RINZU_VALUE_MAP)
        expected += rinzu_managed_map_index_bytes(value->map_index);
      if (value->gc_bytes < expected)
        rinzu_failure_fatal();
      if (value->gc_bytes > expected)
        rinzu_managed_unaccount_payload(data, value, value->gc_bytes - expected);
      if ((kind == RINZU_VALUE_LIST || kind == RINZU_VALUE_MAP) && !value->collection_building)
        value->collection_capacity = 0;
      if (value->slots != &value->inline_slot)
        free(value->slots);
      value->slots = moved;
    }
    value->marked = 0;
    cursor = &value->next;
  }
  rinzu_managed_index_trim(data);
  data->collections += 1;
  rinzu_gc_collected(
      &data->gc,
      rinzu_gc_add(rinzu_gc_add(rinzu_gc_add(data->heap_bytes,
                                             data->small_enums ? RINZU_MANAGED_SMALL_ENUM_CAPACITY *
                                                                     sizeof(*data->small_enums)
                                                               : 0),
                                data->index_capacity * sizeof(*data->index)),
                   data->lookup_cache
                       ? RINZU_MANAGED_LOOKUP_CACHE_CAPACITY * sizeof(*data->lookup_cache)
                       : 0),
      root_bytes);
}
/* A safepoint publishes/validates roots even when no collection is due. Never
   collect inside allocation: results and temporary materializations are not yet
   in a typed frame. Borrowed native views remain forbidden across safepoints. */
/* A nonempty publication validates every handle, even when GC is not due.
   Keep its index walk out of the empty-frame path; the caller has checked
   Owner, native borrowing and the completed publication protocol. */
static __attribute__((noinline)) void
rinzu_managed_validate_frame_owned(rinzu_managed_data_t *data, const rinzu_managed_frame_t *frame) {
  /* No collection, yield or reentry can change these Owner-owned buffers. */
  size_t count = frame->count;
  if (!count)
    return;
  const uint64_t *roots = frame->roots;
  rinzu_managed_lookup_entry_t *cache = data->lookup_cache;
  if (cache) {
    for (size_t index = 0; index < count; ++index) {
      uint64_t identity = roots[index];
      if (identity > RINZU_MANAGED_HANDLE_BASE && identity <= INT64_MAX) {
        /* Full weak membership tag only; no type or reachability proof. */
        if (cache[(size_t)identity & (RINZU_MANAGED_LOOKUP_CACHE_CAPACITY - 1)].identity ==
            identity)
          continue;
        if (!rinzu_managed_lookup_uncached_owned(data, identity))
          rinzu_failure_fatal();
      }
    }
  } else {
    for (size_t index = 0; index < count; ++index) {
      uint64_t identity = roots[index];
      if (identity > RINZU_MANAGED_HANDLE_BASE && identity <= INT64_MAX)
        if (!rinzu_managed_lookup_uncached_owned(data, identity))
          rinzu_failure_fatal();
    }
  }
}
static inline void rinzu_managed_safepoint(rinzu_managed_data_t *data,
                                           const rinzu_managed_roots_t *roots) {
  rinzu_managed_owner(data);
  if (data->live_views || data->view_allocations)
    rinzu_failure_fatal();
  if (roots->current && roots->current->writing)
    rinzu_failure_fatal();
  if (roots->current && roots->current->count)
    rinzu_managed_validate_frame_owned(data, roots->current);
  if (rinzu_gc_due(&data->gc) || (data->future_due && data->future_due(data->future_context)))
    rinzu_managed_collect(data, roots);
}
static inline void rinzu_managed_data_dispose(rinzu_managed_data_t *data) {
  rinzu_managed_owner(data);
  if (data->native_record_loans)
    rinzu_interop_fatal();
  if (data->root_sets || data->published_root_count || data->live_views || data->view_allocations)
    rinzu_failure_fatal();
  int record_evidence = data->native_record_allocations && getenv("RINZU_NATIVE_RECORD_EVIDENCE");
  if (record_evidence)
    fprintf(stderr, "RINZU_NATIVE_RECORD_SCRATCH=%zu,%zu,0,0\n", data->native_record_allocations,
            data->native_record_pool_bytes);
  data->metadata_cache_disposing = 1;
  rinzu_managed_value_t *value = data->values;
  while (value) {
    rinzu_managed_value_t *next = value->next;
    rinzu_managed_value_release(data, value);
    value = next;
  }
  rinzu_managed_index_release(data);
  if (data->literal_accounted_bytes)
    rinzu_resource_release(RINZU_RESOURCE_MANAGED, data->literal_accounted_bytes);
  free(data->literals);
  free(data->byte_literals);
  if (data->small_enum_accounted_bytes)
    rinzu_resource_release(RINZU_RESOURCE_MANAGED, data->small_enum_accounted_bytes);
  free(data->small_enums);
  rinzu_error_reference_t *error = data->error_references;
  while (error) {
    rinzu_error_reference_t *next = error->next;
    error->owner = NULL;
    error->next = NULL;
    if (!error->references)
      free(error);
    error = next;
  }
  while (data->metadata_cache) {
    rinzu_managed_value_t *cached = data->metadata_cache;
    data->metadata_cache = cached->next;
    if (!data->metadata_cache_count)
      rinzu_failure_fatal();
    --data->metadata_cache_count;
    if (cached->accounted_bytes)
      rinzu_resource_release(RINZU_RESOURCE_RUNTIME_CONTROL, cached->accounted_bytes);
    free(cached);
  }
  if (data->metadata_cache_count)
    rinzu_failure_fatal();
  while (data->native_record_pool) {
    rinzu_managed_record_loan_t *cached = data->native_record_pool;
    data->native_record_pool = cached->next;
    if (!cached->storage_bytes || data->native_record_pool_bytes < cached->storage_bytes ||
        cached->identities || cached->count || cached->busy)
      rinzu_interop_fatal();
    data->native_record_pool_bytes -= cached->storage_bytes;
    if (cached->storage_accounted)
      rinzu_resource_release(RINZU_RESOURCE_RUNTIME_CONTROL, cached->storage_bytes);
    free(cached);
  }
  if (data->native_record_pool_bytes)
    rinzu_interop_fatal();
  rinzu_managed_frame_cache_dispose(&data->frame_cache);
  *data = (rinzu_managed_data_t){0};
  if (record_evidence)
    fputs("RINZU_NATIVE_RECORD_DISPOSED=0,0\n", stderr);
}
#endif
