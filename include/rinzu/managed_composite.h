#ifndef RINZU_MANAGED_COMPOSITE_H
#define RINZU_MANAGED_COMPOSITE_H
#include "rinzu/managed_data.h"

static inline int rinzu_managed_configure_values(rinzu_managed_data_t *data,
                                                 const rinzu_value_schema_t *schema) {
  rinzu_managed_owner(data);
  if (!schema || data->schema ||
      (schema->count &&
       (!schema->types || !schema->equality || !schema->transferable || !schema->keys.allowed)))
    return RINZU_VALUE_SCHEMA;
  data->schema = schema;
  return RINZU_VALUE_OK;
}
static inline rinzu_managed_value_t *rinzu_managed_find_optional(rinzu_managed_data_t *data,
                                                                 uint64_t identity) {
  return rinzu_managed_lookup(data, identity);
}
static inline int rinzu_managed_layout(const rinzu_value_type_t *type, int64_t variant,
                                       size_t count) {
  if (type->kind == RINZU_VALUE_ENUM) {
    return variant >= 0 && (uint64_t)variant <= UINT32_MAX &&
                   (uint64_t)variant < type->variant_count && count == type->variants[variant].count
               ? RINZU_VALUE_OK
               : RINZU_VALUE_TYPE;
  }
  if (variant != -1)
    return RINZU_VALUE_TYPE;
  if (type->kind == RINZU_VALUE_RECORD || type->kind == RINZU_VALUE_STATE || type->kind == RINZU_VALUE_ERROR_RECORD)
    return count == type->field_count ? RINZU_VALUE_OK : RINZU_VALUE_TYPE;
  if (type->kind == RINZU_VALUE_LIST)
    return RINZU_VALUE_OK;
  if (type->kind == RINZU_VALUE_MAP)
    return count % 2 ? RINZU_VALUE_TYPE : RINZU_VALUE_OK;
  return RINZU_VALUE_TYPE;
}
static inline size_t rinzu_managed_child_type(const rinzu_value_type_t *type, int64_t variant,
                                              size_t index) {
  if (type->kind == RINZU_VALUE_RECORD || type->kind == RINZU_VALUE_STATE || type->kind == RINZU_VALUE_ERROR_RECORD)
    return type->fields[index];
  if (type->kind == RINZU_VALUE_ENUM)
    return type->variants[variant].types[index];
  if (type->kind == RINZU_VALUE_LIST)
    return type->element;
  return index % 2 ? type->value : type->key;
}
/* Projection, state writes and variant reads require a composite parent.
   Resolve that narrower contract directly, rather than testing every scalar,
   String, Bytes, Error and opaque representation in the general slot resolver.
   Keep the schema, Domain, exact type and complete layout checks. Inline the
   resolvers into projection so checked metadata and temporary output slots
   do not cross separate C calls. */
static inline __attribute__((always_inline)) int
rinzu_managed_composite_resolve_owned(rinzu_managed_data_t *data, size_t type,
                                                        uint64_t identity,
                                                        rinzu_managed_value_t **resolved) {
  *resolved = NULL;
  if (!data->schema || type >= data->schema->count)
    return RINZU_VALUE_SCHEMA;
  rinzu_managed_value_t *value = rinzu_managed_lookup_owned(data, identity);
  if (!value || value->kind != RINZU_MANAGED_COMPOSITE || value->type != type ||
      rinzu_managed_layout(&data->schema->types[type],
                           value->has_variant ? (int64_t)value->variant : -1, value->count))
    return RINZU_VALUE_TYPE;
  *resolved = value;
  return RINZU_VALUE_OK;
}
/* Resolve a typed slot once inside an Owner-checked operation. The resolved
   metadata is valid until the operation returns: no yield/native reentry/GC. */
static inline __attribute__((always_inline)) int
rinzu_managed_slot_resolve_owned(rinzu_managed_data_t *data, size_t type,
                                                   rinzu_managed_slot_t slot,
                                                   rinzu_managed_value_t **resolved) {
  if (resolved)
    *resolved = NULL;
  if (!data->schema || type >= data->schema->count)
    return RINZU_VALUE_SCHEMA;
  if (slot.reference > 1)
    return RINZU_VALUE_TYPE;
  const rinzu_value_type_t *expected = &data->schema->types[type];
  if (!slot.reference) {
    if (expected->kind == RINZU_VALUE_SCALAR)
      return rinzu_transfer_valid(expected->scalar_kind, slot.bits) ? RINZU_VALUE_OK
                                                                    : RINZU_VALUE_TYPE;
    if (expected->kind == RINZU_VALUE_UNIT)
      return slot.bits ? RINZU_VALUE_TYPE : RINZU_VALUE_OK;
    return expected->kind == RINZU_VALUE_OTHER ? RINZU_VALUE_OK : RINZU_VALUE_TYPE;
  }
  rinzu_managed_value_t *value = rinzu_managed_lookup_owned(data, slot.bits);
  if (!value)
    return RINZU_VALUE_TYPE;
  if (resolved)
    *resolved = value;
  if (expected->kind == RINZU_VALUE_ERROR)
    return (value->kind == RINZU_MANAGED_COMPOSITE && value->type < data->schema->count && data->schema->types[value->type].kind == RINZU_VALUE_ERROR_RECORD) || value->kind == RINZU_MANAGED_ERROR ||
                   (value->kind == RINZU_MANAGED_RICH_ERROR &&
                    rinzu_error_value_valid(value->bytes, value->length))
               ? RINZU_VALUE_OK
               : RINZU_VALUE_TYPE;
  if (expected->kind == RINZU_VALUE_STRING)
    return value->kind == RINZU_MANAGED_STRING && value->scalar_length != SIZE_MAX
               ? RINZU_VALUE_OK
               : RINZU_VALUE_TYPE;
  if (expected->kind == RINZU_VALUE_DYNAMIC_BYTES)
    return value->kind == RINZU_MANAGED_BYTES && value->length <= INT64_MAX ? RINZU_VALUE_OK
                                                                            : RINZU_VALUE_TYPE;
  if (expected->kind == RINZU_VALUE_BYTES)
    return value->kind == RINZU_MANAGED_BYTES && value->length == expected->bytes_length
               ? RINZU_VALUE_OK
               : RINZU_VALUE_TYPE;
  if (expected->kind == RINZU_VALUE_OTHER)
    return value->kind == RINZU_MANAGED_ERROR || (value->kind == RINZU_MANAGED_NATIVE_OPAQUE &&
                                                  value->type == type && value->native_pointer)
               ? RINZU_VALUE_OK
               : RINZU_VALUE_TYPE;
  if (expected->kind == RINZU_VALUE_RECORD || expected->kind == RINZU_VALUE_STATE || expected->kind == RINZU_VALUE_ERROR_RECORD ||
      expected->kind == RINZU_VALUE_ENUM || expected->kind == RINZU_VALUE_LIST ||
      expected->kind == RINZU_VALUE_MAP)
    return value->kind == RINZU_MANAGED_COMPOSITE && value->type == type &&
                   !rinzu_managed_layout(
                       expected, value->has_variant ? (int64_t)value->variant : -1, value->count)
               ? RINZU_VALUE_OK
               : RINZU_VALUE_TYPE;
  return RINZU_VALUE_TYPE;
}
static inline int rinzu_managed_slot_valid(rinzu_managed_data_t *data, size_t type,
                                           rinzu_managed_slot_t slot) {
  rinzu_managed_owner(data);
  return rinzu_managed_slot_resolve_owned(data, type, slot, NULL);
}
/* Build a fresh immutable result in its final payload. The caller has checked
   the Owner and must finish filling it before a yield, reentry or safepoint. */
static inline rinzu_managed_value_t *
rinzu_managed_bytes_allocate_owned(rinzu_managed_data_t *data, size_t length) {
  if (length == SIZE_MAX)
    rinzu_failure_fatal();
  rinzu_managed_value_t *value = rinzu_managed_new_owned(data, RINZU_MANAGED_BYTES);
  value->length = length;
  if (data->inline_bytes_enabled && length < sizeof(value->inline_bytes))
    value->bytes = value->inline_bytes;
  else {
    value->bytes = rinzu_failure_allocate(length + 1);
    rinzu_managed_account_payload(data, value, length + 1);
  }
  return value;
}
/* Only the generated constant helper supplies immutable program-lifetime
   storage. Ordinary/native inputs and explicit bytesCopy still allocate their
   own snapshots; fixed-length padding remains a separate operation. */
static inline uint64_t rinzu_managed_bytes_literal(rinzu_managed_data_t *data,
                                                   const unsigned char *bytes, size_t length) {
  rinzu_managed_owner(data);
  if (length == SIZE_MAX || (length && !bytes))
    rinzu_failure_fatal();
  uint64_t hit;
  rinzu_managed_literal_t *literal;
  if (rinzu_managed_literal_lookup_owned(
          data, &data->byte_literals, &data->byte_literal_victim, bytes, length, &literal, &hit))
    return hit;
  rinzu_managed_value_t *value = rinzu_managed_bytes_allocate_owned(data, length);
  if (length)
    memcpy(value->bytes, bytes, length);
  *literal = (rinzu_managed_literal_t){bytes, length, value->identity};
  return value->identity;
}
static inline int rinzu_managed_bytes(rinzu_managed_data_t *data, uint64_t expected,
                                      const unsigned char *bytes, size_t length, uint64_t *output) {
  rinzu_managed_owner(data);
  if (!output || expected != length || length == SIZE_MAX || (length && !bytes))
    return RINZU_VALUE_TYPE;
  rinzu_managed_value_t *value = rinzu_managed_bytes_allocate_owned(data, length);
  if (length)
    memcpy(value->bytes, bytes, length);
  *output = value->identity;
  return RINZU_VALUE_OK;
}

typedef struct rinzu_managed_view_allocation {
  void *pointer;
  struct rinzu_managed_view_allocation *next;
} rinzu_managed_view_allocation_t;
/* Field arrays are native-owned; String/Bytes payloads are borrowed from rooted
   Owner data. The view must be disposed before any GC or Domain teardown. */
typedef struct {
  rinzu_value_t value;
  rinzu_managed_view_allocation_t *allocations;
  rinzu_managed_data_t *owner;
  int initialized;
} rinzu_managed_snapshot_t;
static inline void rinzu_managed_snapshot_dispose(rinzu_managed_snapshot_t *view) {
  if (view->owner)
    rinzu_managed_owner(view->owner);
  rinzu_managed_view_allocation_t *allocation = view->allocations;
  while (allocation) {
    if (!view->owner || view->owner->view_allocations < 2)
      rinzu_failure_fatal();
    rinzu_managed_view_allocation_t *next = allocation->next;
    free(allocation->pointer);
    free(allocation);
    view->owner->view_allocations -= 2;
    allocation = next;
  }
  if (view->owner) {
    if (!view->owner->live_views)
      rinzu_failure_fatal();
    view->owner->live_views -= 1;
  }
  *view = (rinzu_managed_snapshot_t){0};
}
static inline void *rinzu_managed_snapshot_allocate(rinzu_managed_snapshot_t *view, size_t count,
                                                    size_t width) {
  if (!width || count > SIZE_MAX / width)
    rinzu_failure_fatal();
  if (!count)
    return NULL;
  if (!view->owner || view->owner->view_allocations > SIZE_MAX - 2)
    rinzu_failure_fatal();
  void *pointer = rinzu_failure_allocate(count * width);
  rinzu_managed_view_allocation_t *allocation = rinzu_failure_allocate(sizeof(*allocation));
  view->owner->view_allocations += 2;
  allocation->pointer = pointer;
  allocation->next = view->allocations;
  view->allocations = allocation;
  return pointer;
}
typedef struct {
  size_t type;
  rinzu_managed_slot_t slot;
  rinzu_value_t *output;
} rinzu_managed_view_step_t;
static inline void rinzu_managed_view_push(rinzu_managed_view_step_t **stack, size_t *depth,
                                           size_t *capacity, rinzu_managed_view_step_t step) {
  if (*depth == *capacity) {
    if (*capacity > SIZE_MAX / 2 / sizeof(**stack))
      rinzu_failure_fatal();
    size_t next = *capacity ? *capacity * 2 : 16;
    rinzu_managed_view_step_t *grown = rinzu_failure_allocate(next * sizeof(*grown));
    if (*depth)
      memcpy(grown, *stack, *depth * sizeof(*grown));
    free(*stack);
    *stack = grown;
    *capacity = next;
  }
  (*stack)[(*depth)++] = step;
}
static inline int rinzu_managed_snapshot(rinzu_managed_data_t *data, size_t type,
                                         rinzu_managed_slot_t slot,
                                         rinzu_managed_snapshot_t *destination) {
  rinzu_managed_owner(data);
  if (!destination || destination->initialized || destination->allocations || destination->owner)
    return RINZU_VALUE_TYPE;
  int status = rinzu_managed_slot_resolve_owned(data, type, slot, NULL);
  if (status)
    return status;
  rinzu_managed_snapshot_t prepared = {0};
  if (data->live_views == SIZE_MAX)
    rinzu_failure_fatal();
  data->live_views += 1;
  prepared.owner = data;
  rinzu_managed_view_step_t *stack = NULL;
  size_t depth = 0, capacity = 0;
  rinzu_managed_view_push(&stack, &depth, &capacity,
                          (rinzu_managed_view_step_t){type, slot, &prepared.value});
  while (depth && !status) {
    rinzu_managed_view_step_t step = stack[--depth];
    /* This synchronous walk cannot yield, reenter managed code or collect.
       Reuse the Owner check and the validated metadata for this same step. */
    rinzu_managed_value_t *value = NULL;
    status = rinzu_managed_slot_resolve_owned(data, step.type, step.slot, &value);
    if (status)
      break;
    const rinzu_value_type_t *descriptor = &data->schema->types[step.type];
    rinzu_value_t *view = step.output;
    // Outputs are &prepared.value or children of a nonempty allocated snapshot.
    // The analyzer loses this invariant when the explicit work stack grows.
    // NOLINTNEXTLINE(clang-analyzer-core.NullDereference)
    view->kind = descriptor->kind;
    if (descriptor->kind == RINZU_VALUE_SCALAR) {
      view->scalar_kind = descriptor->scalar_kind;
      view->bits = step.slot.bits;
      continue;
    }
    if (descriptor->kind == RINZU_VALUE_UNIT)
      continue;
    if (descriptor->kind == RINZU_VALUE_STATE || descriptor->kind == RINZU_VALUE_ERROR_RECORD) {
      view->other_type = step.type;
      view->bits = step.slot.bits;
      continue;
    }
    if (descriptor->kind == RINZU_VALUE_OTHER) {
      if (step.slot.reference) {
        status = RINZU_VALUE_TYPE;
        break;
      }
      view->other_type = step.type;
      view->bits = step.slot.bits;
      continue;
    }
    if (!value)
      rinzu_failure_fatal();
    if (descriptor->kind == RINZU_VALUE_ERROR && value->kind == RINZU_MANAGED_COMPOSITE && data->schema->types[value->type].kind == RINZU_VALUE_ERROR_RECORD) {
      view->kind = RINZU_VALUE_ERROR_RECORD;
      view->other_type = value->type;
      view->bits = step.slot.bits;
      continue;
    }
    if (descriptor->kind == RINZU_VALUE_ERROR) {
      rinzu_error_buffer_t blob = rinzu_managed_error_blob(data, value);
      unsigned char *copy = rinzu_managed_snapshot_allocate(&prepared, blob.length, 1);
      memcpy(copy, blob.bytes, blob.length);
      view->bytes = copy;
      view->length = blob.length;
      free(blob.bytes);
      continue;
    }
    if (descriptor->kind == RINZU_VALUE_STRING || descriptor->kind == RINZU_VALUE_BYTES ||
        descriptor->kind == RINZU_VALUE_DYNAMIC_BYTES) {
      view->bytes = value->bytes;
      view->length = value->length;
      continue;
    }
    int64_t variant = value->has_variant ? (int64_t)value->variant : -1;
    view->identity = descriptor->identity;
    view->variant = value->variant;
    if (descriptor->kind == RINZU_VALUE_MAP) {
      view->count = value->count / 2;
      rinzu_value_pair_t *entries =
          rinzu_managed_snapshot_allocate(&prepared, view->count, sizeof(*entries));
      view->entries = entries;
      for (size_t index = value->count; index; --index) {
        size_t field = index - 1;
        rinzu_value_t *child = field % 2 ? &entries[field / 2].value : &entries[field / 2].key;
        rinzu_managed_view_push(
            &stack, &depth, &capacity,
            (rinzu_managed_view_step_t){rinzu_managed_child_type(descriptor, variant, field),
                                        value->slots[field], child});
      }
    } else {
      view->count = value->count;
      rinzu_value_t *fields =
          rinzu_managed_snapshot_allocate(&prepared, view->count, sizeof(*fields));
      view->fields = fields;
      for (size_t index = value->count; index; --index) {
        size_t field = index - 1;
        rinzu_managed_view_push(
            &stack, &depth, &capacity,
            (rinzu_managed_view_step_t){rinzu_managed_child_type(descriptor, variant, field),
                                        value->slots[field], &fields[field]});
      }
    }
  }
  free(stack);
  if (!status)
    status = rinzu_value_validate(data->schema, type, &prepared.value);
  if (status) {
    rinzu_managed_snapshot_dispose(&prepared);
    return status;
  }
  prepared.initialized = 1;
  *destination = prepared;
  return RINZU_VALUE_OK;
}
static inline int rinzu_managed_values_equal(rinzu_managed_data_t *data, size_t type,
                                             rinzu_managed_slot_t left, rinzu_managed_slot_t right,
                                             int *output) {
  rinzu_managed_owner(data);
  if (!data->schema || type >= data->schema->count)
    return RINZU_VALUE_SCHEMA;
  if (!data->schema->equality[type])
    return RINZU_VALUE_NO_EQUALITY;
  if (!output)
    return RINZU_VALUE_TYPE;
  const rinzu_value_type_t *layout = &data->schema->types[type];
  if (layout->kind == RINZU_VALUE_SCALAR || layout->kind == RINZU_VALUE_UNIT) {
    int status = rinzu_managed_slot_resolve_owned(data, type, left, NULL);
    if (!status)
      status = rinzu_managed_slot_resolve_owned(data, type, right, NULL);
    if (!status)
      *output = layout->kind == RINZU_VALUE_UNIT ||
                rinzu_scalar_compare(layout->scalar_kind, 0, left.bits, right.bits);
    return status;
  }
  if (layout->kind == RINZU_VALUE_STRING || layout->kind == RINZU_VALUE_BYTES ||
      layout->kind == RINZU_VALUE_DYNAMIC_BYTES) {
    rinzu_managed_value_t *a = NULL, *b = NULL;
    int status = rinzu_managed_slot_resolve_owned(data, type, left, &a);
    if (!status)
      status = rinzu_managed_slot_resolve_owned(data, type, right, &b);
    if (!status)
      *output = a->length == b->length &&
                (!a->length || !memcmp(a->bytes, b->bytes, a->length));
    return status;
  }
  rinzu_managed_snapshot_t a = {0}, b = {0};
  int status = rinzu_managed_snapshot(data, type, left, &a);
  if (!status)
    status = rinzu_managed_snapshot(data, type, right, &b);
  if (!status)
    status = rinzu_value_equal(data->schema, type, &a.value, &b.value, output);
  rinzu_managed_snapshot_dispose(&a);
  rinzu_managed_snapshot_dispose(&b);
  return status;
}
static inline int rinzu_managed_value_hash(rinzu_managed_data_t *data, size_t type,
                                           rinzu_managed_slot_t slot, uint64_t *output) {
  rinzu_managed_owner(data);
  if (!data->schema || type >= data->schema->count)
    return RINZU_VALUE_SCHEMA;
  if (!data->schema->keys.allowed[type])
    return RINZU_VALUE_NOT_KEY;
  if (!output)
    return RINZU_VALUE_TYPE;
  const rinzu_value_type_t *layout = &data->schema->types[type];
  if (layout->kind == RINZU_VALUE_SCALAR || layout->kind == RINZU_VALUE_STRING ||
      layout->kind == RINZU_VALUE_BYTES || layout->kind == RINZU_VALUE_DYNAMIC_BYTES) {
    rinzu_managed_value_t *value = NULL;
    int status = rinzu_managed_slot_resolve_owned(data, type, slot, &value);
    if (status)
      return status;
    rinzu_key_hasher_t hash = rinzu_key_init();
    if (layout->kind == RINZU_VALUE_SCALAR) {
      if (layout->scalar_kind == RINZU_SCALAR_BOOL)
        (void)rinzu_key_bool(&hash, slot.bits);
      else
        (void)rinzu_key_integer(&hash, layout->scalar_kind, slot.bits);
    } else {
      /* String validity is immutable metadata checked by slot resolution. */
      const unsigned char prefix = layout->kind == RINZU_VALUE_STRING ? 0x20 : 0x21;
      rinzu_key_write(&hash, &prefix, 1);
      rinzu_key_u64(&hash, value->length, 8);
      rinzu_key_write(&hash, value->bytes, value->length);
    }
    *output = hash.hash;
    return RINZU_VALUE_OK;
  }
  rinzu_managed_snapshot_t view = {0};
  int status = rinzu_managed_snapshot(data, type, slot, &view);
  if (!status)
    status = rinzu_value_key_hash(data->schema, type, &view.value, output);
  rinzu_managed_snapshot_dispose(&view);
  return status;
}
static inline rinzu_managed_map_index_t *rinzu_managed_map_index_new(size_t pairs) {
  if (!pairs)
    return NULL;
  size_t capacity = 4;
  while (pairs > capacity / 2) {
    if (capacity > SIZE_MAX / 2)
      rinzu_failure_fatal();
    capacity *= 2;
  }
  if (capacity > (SIZE_MAX - sizeof(rinzu_managed_map_index_t)) /
                     sizeof(rinzu_managed_map_bucket_t))
    rinzu_failure_fatal();
  rinzu_managed_map_index_t *index = rinzu_failure_allocate(
      sizeof(*index) + capacity * sizeof(*index->buckets));
  index->capacity = capacity;
  return index;
}
/* The caller has checked Owner and the complete operation cannot collect. */
static inline int rinzu_managed_map_probe_owned(
    rinzu_managed_data_t *data, size_t key_type, const rinzu_managed_slot_t *slots,
    rinzu_managed_map_index_t *index, rinzu_managed_slot_t key, uint64_t hash,
    size_t *bucket, size_t *selected) {
  *selected = SIZE_MAX;
  *bucket = 0;
  if (!index)
    return RINZU_VALUE_OK;
  size_t cursor = (size_t)hash & (index->capacity - 1);
  while (index->buckets[cursor].slot_plus_one) {
    rinzu_managed_map_bucket_t candidate = index->buckets[cursor];
    if (candidate.hash == hash) {
      int equal = 0;
      int status = rinzu_managed_values_equal(
          data, key_type, slots[candidate.slot_plus_one - 1], key, &equal);
      if (status)
        return status;
      if (equal) {
        *selected = candidate.slot_plus_one - 1;
        break;
      }
    }
    cursor = (cursor + 1) & (index->capacity - 1);
  }
  *bucket = cursor;
  return RINZU_VALUE_OK;
}
static inline void rinzu_managed_map_index_add(rinzu_managed_map_index_t *index,
                                               uint64_t hash, size_t slot) {
  size_t bucket = (size_t)hash & (index->capacity - 1);
  while (index->buckets[bucket].slot_plus_one)
    bucket = (bucket + 1) & (index->capacity - 1);
  index->buckets[bucket] = (rinzu_managed_map_bucket_t){hash, slot + 1};
}
/* Called after layout and every typed child validation. This weak table never
   marks a value, retains a metadata pointer, yields or collects. */
#if RINZU_MANAGED_SMALL_ENUM_MODE
static inline __attribute__((always_inline)) rinzu_managed_small_enum_t *rinzu_managed_small_enum_slot_owned(
    rinzu_managed_data_t *data, const rinzu_value_type_t *descriptor, size_t type,
    uint32_t variant, const rinzu_managed_slot_t *slots, size_t count,
    uint16_t *tag, int allow_empty_list) {
#if RINZU_MANAGED_SMALL_ENUM_MODE < 2
  (void)slots;
#endif
  if (descriptor->kind != RINZU_VALUE_ENUM &&
      !(allow_empty_list && descriptor->kind == RINZU_VALUE_LIST && count == 0))
    return NULL;
  *tag = 0;
  if (count) {
#if RINZU_MANAGED_SMALL_ENUM_MODE >= 2
    const rinzu_value_type_t *child = &data->schema->types[descriptor->variants[variant].types[0]];
    if (child->kind != RINZU_VALUE_SCALAR ||
        (child->scalar_kind != RINZU_SCALAR_U8 && child->scalar_kind != RINZU_SCALAR_BOOL)) return NULL;
    *tag = (uint16_t)(slots[0].bits + 1);
#else
    return NULL;
#endif
  }
  if (!data->small_enums) {
    size_t bytes = RINZU_MANAGED_SMALL_ENUM_CAPACITY * sizeof(*data->small_enums);
    data->small_enums = rinzu_failure_allocate(bytes);
    rinzu_gc_allocated(&data->gc, bytes);
    if (atomic_load_explicit(&rinzu_resource_enabled, memory_order_relaxed)) {
      rinzu_resource_acquire(RINZU_RESOURCE_MANAGED, bytes);
      data->small_enum_accounted_bytes = bytes;
    }
  }
  uint64_t key = (uint64_t)type ^ ((uint64_t)variant << 32) ^ ((uint64_t)*tag << 48);
  return &data->small_enums[rinzu_managed_index_bucket(key, RINZU_MANAGED_SMALL_ENUM_CAPACITY)];
}
#endif

static inline int rinzu_managed_composite_impl_owned(rinzu_managed_data_t *data, size_t type,
                                                int64_t variant, const rinzu_managed_slot_t *slots,
                                                size_t count, uint64_t *output, int allow_cache) {
  if (!data->schema || type >= data->schema->count)
    return RINZU_VALUE_SCHEMA;
  if (!output || (count && !slots) || count > SIZE_MAX / sizeof(*slots))
    return RINZU_VALUE_TYPE;
  const rinzu_value_type_t *descriptor = &data->schema->types[type];
  int status = rinzu_managed_layout(descriptor, variant, count);
  if (status)
    return status;
  if (descriptor->kind == RINZU_VALUE_MAP &&
      (count % 2 != 0 || count / 2 > SIZE_MAX / sizeof(rinzu_managed_snapshot_t)))
    return RINZU_VALUE_TYPE;
  for (size_t index = 0; index < count; ++index) {
    status = rinzu_managed_slot_resolve_owned(
        data, rinzu_managed_child_type(descriptor, variant, index), slots[index], NULL);
    if (status)
      return status;
  }
  /* A one-child Option/result/record needs no separate slots allocation. */
  if (descriptor->kind != RINZU_VALUE_MAP && count <= 1) {
#if RINZU_MANAGED_SMALL_ENUM_MODE
    uint16_t tag = 0;
    rinzu_managed_small_enum_t *cached = allow_cache ?
        rinzu_managed_small_enum_slot_owned(data, descriptor, type, (uint32_t)variant, slots, count, &tag,
                                            (allow_cache & 2) != 0) : NULL;
    if (cached && cached->identity && cached->type == type &&
        cached->variant == (uint32_t)variant && cached->tag == tag &&
        rinzu_managed_lookup_owned(data, cached->identity)) {
      *output = cached->identity;
      return RINZU_VALUE_OK;
    }
#else
    (void)allow_cache;
#endif
    rinzu_managed_value_t *value = rinzu_managed_new_owned(data, RINZU_MANAGED_COMPOSITE);
    value->type = type;
    value->has_variant = variant != -1;
    value->variant = variant == -1 ? 0 : (uint32_t)variant;
    value->count = count;
    if (count) {
      value->inline_slot = slots[0];
      value->slots = &value->inline_slot;
    }
#if RINZU_MANAGED_SMALL_ENUM_MODE
    if (cached) *cached = (rinzu_managed_small_enum_t){type, value->identity, (uint32_t)variant, tag};
#endif
    *output = value->identity;
    return RINZU_VALUE_OK;
  }
  rinzu_managed_slot_t *copy = count ? rinzu_failure_allocate(count * sizeof(*copy)) : NULL;
  if (count)
    memcpy(copy, slots, count * sizeof(*copy));
  size_t used = count;
  rinzu_managed_map_index_t *map_index = NULL;
  if (descriptor->kind == RINZU_VALUE_MAP) {
    used = 0;
    map_index = rinzu_managed_map_index_new(count / 2);
    for (size_t index = 0; index < count && !status; index += 2) {
      uint64_t hash = 0;
      size_t bucket = 0, previous = SIZE_MAX;
      status = rinzu_managed_value_hash(data, descriptor->key, slots[index], &hash);
      if (!status)
        status = rinzu_managed_map_probe_owned(data, descriptor->key, copy, map_index,
                                               slots[index], hash, &bucket, &previous);
      if (!status && previous == SIZE_MAX) {
        copy[used] = slots[index];
        copy[used + 1] = slots[index + 1];
        map_index->buckets[bucket] = (rinzu_managed_map_bucket_t){hash, used + 1};
        used += 2;
      } else if (!status) {
        copy[previous] = slots[index];
        copy[previous + 1] = slots[index + 1];
      }
    }
  }
  if (status) {
    free(copy);
    free(map_index);
    return status;
  }
  if (!used) {
    free(copy);
    copy = NULL;
  }
  rinzu_managed_value_t *value = rinzu_managed_new_owned(data, RINZU_MANAGED_COMPOSITE);
  value->type = type;
  value->has_variant = variant != -1;
  value->variant = variant == -1 ? 0 : (uint32_t)variant;
  value->slots = copy;
  value->count = used;
  if (descriptor->kind == RINZU_VALUE_MAP) {
    value->map_index = map_index;
    rinzu_managed_account_payload(data, value, rinzu_managed_map_index_bytes(map_index));
  }
  if (copy)
    rinzu_managed_account_payload(data, value, count * sizeof(*copy));
  *output = value->identity;
  return RINZU_VALUE_OK;
}
/* Normal constructors retain the caller's existing logical allocation charge. */
static inline int rinzu_managed_composite_owned(rinzu_managed_data_t *data, size_t type,
    int64_t variant, const rinzu_managed_slot_t *slots, size_t count, uint64_t *output) {
  return rinzu_managed_composite_impl_owned(data, type, variant, slots, count, output, 1);
}
/* Only the explicit Source constructor entry has a fixed logical charge and
   may share an empty immutable List. Generic/fresh materialization keeps its
   existing physical-delta accounting. Full layout/child/Owner checks remain. */
static inline int rinzu_managed_constructor_charged(rinzu_managed_data_t *data, size_t type,
    int64_t variant, const rinzu_managed_slot_t *slots, size_t count, uint64_t *output) {
  rinzu_managed_owner(data);
  return rinzu_managed_composite_impl_owned(data, type, variant, slots, count, output, 3);
}
/* Physical-object-delta accounting in Error materialization must keep fresh
   metadata. An explicit argument avoids mutable cache-suspension state. */
static inline int rinzu_managed_composite_fresh(rinzu_managed_data_t *data, size_t type,
    int64_t variant, const rinzu_managed_slot_t *slots, size_t count, uint64_t *output) {
  rinzu_managed_owner(data);
  return rinzu_managed_composite_impl_owned(data, type, variant, slots, count, output, 0);
}

/* Private List update. The caller validated the exact List/argument types and
   Owner; source slots are immutable and already valid. selected==SIZE_MAX is
   append, otherwise selected is in bounds. No yield/reentry/GC may occur until
   the new result (and its Option, if any) is published by the caller. */
static inline uint64_t rinzu_managed_list_update_owned(
    rinzu_managed_data_t *data, const rinzu_managed_value_t *source, size_t selected,
    rinzu_managed_slot_t replacement, int remove) {
  size_t count = source->count;
  if (remove) --count;
  else if (selected == SIZE_MAX) {
    if (count >= SIZE_MAX / sizeof(rinzu_managed_slot_t)) rinzu_failure_fatal();
    ++count;
  }
  rinzu_managed_value_t *value = rinzu_managed_new_owned(data, RINZU_MANAGED_COMPOSITE);
  value->type = source->type;
  value->count = count;
  if (count == 1) {
    value->inline_slot = remove ? source->slots[selected ? 0 : 1] : replacement;
    value->slots = &value->inline_slot;
  } else if (count > 1) {
    rinzu_managed_slot_t *slots = rinzu_failure_allocate(count * sizeof(*slots));
    if (remove) {
      if (selected) memcpy(slots, source->slots, selected * sizeof(*slots));
      if (count - selected)
        memcpy(slots + selected, source->slots + selected + 1,
               (count - selected) * sizeof(*slots));
    } else {
      if (source->count) memcpy(slots, source->slots, source->count * sizeof(*slots));
      slots[selected == SIZE_MAX ? count - 1 : selected] = replacement;
    }
    value->slots = slots;
    rinzu_managed_account_payload(data, value, count * sizeof(*slots));
  }
  return value->identity;
}
/* Only the validated immutable Map operation uses this path. Existing slots and
   hashes are already canonical; the caller validates the new typed arguments.
   Copy once, preserve insertion order, and rebuild offsets without rehashing or
   renormalizing the old keys. No yield/GC occurs before publishing the result. */
static inline uint64_t rinzu_managed_map_update_owned(
    rinzu_managed_data_t *data, const rinzu_managed_value_t *source, size_t selected,
    rinzu_managed_slot_t key, rinzu_managed_slot_t replacement, uint64_t hash, int remove) {
  size_t count = source->count;
  if (remove && selected != SIZE_MAX)
    count -= 2;
  else if (!remove && selected == SIZE_MAX) {
    if (count > SIZE_MAX / sizeof(rinzu_managed_slot_t) - 2)
      rinzu_failure_fatal();
    count += 2;
  }
  rinzu_managed_slot_t *slots = count ? rinzu_failure_allocate(count * sizeof(*slots)) : NULL;
  if (remove && selected != SIZE_MAX) {
    if (selected)
      memcpy(slots, source->slots, selected * sizeof(*slots));
    if (source->count - selected - 2)
      memcpy(slots + selected, source->slots + selected + 2,
             (source->count - selected - 2) * sizeof(*slots));
  } else {
    if (source->count)
      memcpy(slots, source->slots, source->count * sizeof(*slots));
    if (!remove) {
      if (selected == SIZE_MAX)
        slots[count - 2] = key;
      slots[selected == SIZE_MAX ? count - 1 : selected + 1] = replacement;
    }
  }
  rinzu_managed_map_index_t *index = rinzu_managed_map_index_new(count / 2);
  if (source->map_index)
    for (size_t i = 0; i < source->map_index->capacity; ++i) {
      rinzu_managed_map_bucket_t old = source->map_index->buckets[i];
      if (!old.slot_plus_one)
        continue;
      size_t slot = old.slot_plus_one - 1;
      if (remove && slot == selected)
        continue;
      if (remove && selected != SIZE_MAX && slot > selected)
        slot -= 2;
      rinzu_managed_map_index_add(index, old.hash, slot);
    }
  if (!remove && selected == SIZE_MAX)
    rinzu_managed_map_index_add(index, hash, count - 2);
  rinzu_managed_value_t *value = rinzu_managed_new_owned(data, RINZU_MANAGED_COMPOSITE);
  value->type = source->type;
  value->slots = slots;
  value->count = count;
  value->map_index = index;
  rinzu_managed_account_payload(data, value, count * sizeof(*slots) +
                                             rinzu_managed_map_index_bytes(index));
  return value->identity;
}
static inline int rinzu_managed_composite(rinzu_managed_data_t *data, size_t type, int64_t variant,
                                          const rinzu_managed_slot_t *slots, size_t count,
                                          uint64_t *output) {
  rinzu_managed_owner(data);
  return rinzu_managed_composite_owned(data, type, variant, slots, count, output);
}
typedef struct {
  size_t type, field, count;
  const rinzu_value_t *value;
  rinzu_managed_slot_t *slots, *output;
  unsigned entered;
} rinzu_managed_copy_frame_t;
/* Validate the whole vector before allocating. Publish all returned reference
   slots in the receiver frame before any safepoint or transport settlement. */
static inline int rinzu_managed_materialize_values(rinzu_managed_data_t *data, const size_t *types,
                                                   const rinzu_value_t *values, size_t count,
                                                   rinzu_managed_slot_t *output) {
  rinzu_managed_owner(data);
  if (!data->schema || (count && (!types || !values || !output)) ||
      count > SIZE_MAX / sizeof(*output) ||
      data->schema->count > SIZE_MAX / sizeof(rinzu_managed_copy_frame_t))
    return RINZU_VALUE_SCHEMA;
  for (size_t index = 0; index < count; ++index) {
    if (types[index] >= data->schema->count)
      return RINZU_VALUE_SCHEMA;
    if (!data->schema->transferable[types[index]])
      return RINZU_VALUE_NO_EQUALITY;
    int status = rinzu_value_validate(data->schema, types[index], &values[index]);
    if (status)
      return status;
  }
  rinzu_managed_slot_t *prepared = count ? rinzu_failure_allocate(count * sizeof(*prepared)) : NULL;
  rinzu_managed_copy_frame_t *stack =
      count ? rinzu_failure_allocate(data->schema->count * sizeof(*stack)) : NULL;
  rinzu_managed_value_t *before = data->values;
  int status = RINZU_VALUE_OK;
  size_t depth = 0;
  for (size_t root = 0; root < count && !status; ++root) {
    depth = 1;
    stack[0] = (rinzu_managed_copy_frame_t){
        .type = types[root], .value = &values[root], .output = &prepared[root]};
    while (depth && !status) {
      rinzu_managed_copy_frame_t *frame = &stack[depth - 1];
      const rinzu_value_type_t *type = &data->schema->types[frame->type];
      const rinzu_value_t *value = frame->value;
      if (!frame->entered) {
        frame->entered = 1;
        if (type->kind == RINZU_VALUE_SCALAR || type->kind == RINZU_VALUE_UNIT) {
          *frame->output =
              (rinzu_managed_slot_t){type->kind == RINZU_VALUE_UNIT ? 0 : value->bits, 0};
          depth -= 1;
          continue;
        }
        if (type->kind == RINZU_VALUE_ERROR) {
          *frame->output = (rinzu_managed_slot_t){
              rinzu_managed_rich_error(data, value->bytes, value->length), 1};
          depth -= 1;
          continue;
        }
        if (type->kind == RINZU_VALUE_STRING || type->kind == RINZU_VALUE_BYTES ||
            type->kind == RINZU_VALUE_DYNAMIC_BYTES) {
          uint64_t bits = 0;
          if (type->kind == RINZU_VALUE_STRING)
            bits = rinzu_managed_string(data, value->bytes, value->length);
          else
            status = rinzu_managed_bytes(
                data, type->kind == RINZU_VALUE_DYNAMIC_BYTES ? value->length : type->bytes_length,
                value->bytes, value->length, &bits);
          if (!status)
            *frame->output = (rinzu_managed_slot_t){bits, 1};
          depth -= 1;
          continue;
        }
        frame->count = type->kind == RINZU_VALUE_MAP ? value->count * 2 : value->count;
        if (frame->count > SIZE_MAX / sizeof(*frame->slots))
          rinzu_failure_fatal();
        frame->slots =
            frame->count ? rinzu_failure_allocate(frame->count * sizeof(*frame->slots)) : NULL;
      }
      int64_t variant = type->kind == RINZU_VALUE_ENUM ? (int64_t)value->variant : -1;
      if (frame->field == frame->count) {
        uint64_t bits = 0;
        status =
            rinzu_managed_composite(data, frame->type, variant, frame->slots, frame->count, &bits);
        if (!status)
          *frame->output = (rinzu_managed_slot_t){bits, 1};
        free(frame->slots);
        frame->slots = NULL;
        depth -= 1;
        continue;
      }
      size_t field = frame->field++, child = rinzu_managed_child_type(type, variant, field);
      const rinzu_value_t *next =
          type->kind == RINZU_VALUE_MAP
              ? (field & 1 ? &value->entries[field / 2].value : &value->entries[field / 2].key)
              : &value->fields[field];
      if (depth == data->schema->count)
        rinzu_failure_fatal();
      stack[depth++] = (rinzu_managed_copy_frame_t){
          .type = child, .value = next, .output = &frame->slots[field]};
    }
  }
  while (depth)
    free(stack[--depth].slots);
  free(stack);
  if (status) {
    while (data->values != before) {
      rinzu_managed_value_t *value = data->values;
      data->values = value->next;
      rinzu_managed_index_remove(data, value);
      rinzu_managed_value_release(data, value);
      data->objects -= 1;
    }
    /* Rollback remains allocation-free; collection can shrink a live index. */
    if (!data->objects)
      rinzu_managed_index_trim(data);
  } else if (count)
    memcpy(output, prepared, count * sizeof(*output));
  free(prepared);
  return status;
}
/* Publish the returned reference in the caller's typed frame before its next
   safepoint. The parent may be released once that publication is complete. */
static inline __attribute__((always_inline)) int
rinzu_managed_project_owned(rinzu_managed_data_t *data, uint64_t identity, size_t type,
                                        int64_t variant, size_t field,
                                        rinzu_managed_slot_t *output) {
  rinzu_managed_value_t *value = NULL;
  int status =
      rinzu_managed_composite_resolve_owned(data, type, identity, &value);
  if (status)
    return status;
  if (!output || !value || value->kind != RINZU_MANAGED_COMPOSITE ||
      (value->has_variant ? (int64_t)value->variant : -1) != variant || field >= value->count)
    return RINZU_VALUE_TYPE;
  if (rinzu_managed_record_busy(data, identity))
    return RINZU_VALUE_BUSY;
  status = rinzu_managed_slot_resolve_owned(
      data, rinzu_managed_child_type(&data->schema->types[type], variant, field),
      value->slots[field], NULL);
  if (status)
    return status;
  *output = value->slots[field];
  return RINZU_VALUE_OK;
}
static inline __attribute__((always_inline)) int
rinzu_managed_project(rinzu_managed_data_t *data, uint64_t identity, size_t type,
                      int64_t variant, size_t field, rinzu_managed_slot_t *output) {
  rinzu_managed_owner(data);
  return rinzu_managed_project_owned(data, identity, type, variant, field, output);
}
/* No concurrent/incremental collection: this owner-only barrier validates the
   edge before publishing it; every collection traces the full rooted graph. */
static inline int rinzu_managed_set_state_field(rinzu_managed_data_t *data, uint64_t identity,
                                                size_t type, size_t field,
                                                rinzu_managed_slot_t slot) {
  rinzu_managed_owner(data);
  rinzu_managed_value_t *value = NULL;
  int status =
      rinzu_managed_composite_resolve_owned(data, type, identity, &value);
  if (status)
    return status;
  const rinzu_value_type_t *layout = &data->schema->types[type];
  if (!value || layout->kind != RINZU_VALUE_STATE || field >= layout->field_count)
    return RINZU_VALUE_TYPE;
  status = rinzu_managed_slot_resolve_owned(data, layout->fields[field], slot, NULL);
  if (status)
    return status;
  value->slots[field] = slot;
  return RINZU_VALUE_OK;
}
static inline int rinzu_managed_enum_variant(rinzu_managed_data_t *data, uint64_t identity,
                                             size_t type, uint32_t *output) {
  rinzu_managed_owner(data);
  rinzu_managed_value_t *value = NULL;
  int status =
      rinzu_managed_composite_resolve_owned(data, type, identity, &value);
  if (status)
    return status;
  if (!output || !value || data->schema->types[type].kind != RINZU_VALUE_ENUM ||
      !value->has_variant)
    return RINZU_VALUE_TYPE;
  *output = value->variant;
  return RINZU_VALUE_OK;
}
#endif
